use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;
use manatan_sdk::{
    client::{Client, BROWSER_USER_AGENT},
    runtime, CatalogItem, Error, FilterDefinition, ImageRequest, MangaChapter, MangaPage,
    MangaSource, OptionItem, PageContent, Paged, Result, UrlResolveResult,
};
use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};
use url::Url;

const BASE_URL: &str = "https://3asq.online";
const REFERER: &str = "https://3asq.online/";
const LANGUAGE: &str = "ar";
const CONTENT_RATING: &str = "suggestive";
const REQUEST_LIMIT_MS: u32 = 250;
const MAX_DOCUMENT_BYTES: u64 = 8_000_000;
const MAX_CHAPTER_LIST_BYTES: u64 = 12_000_000;

pub struct Manga3asqSource {
    client: Client,
}

impl Default for Manga3asqSource {
    fn default() -> Self {
        Self {
            client: Client::browser()
                .cookies_for(BASE_URL)
                .header("Referer", REFERER),
        }
    }
}

impl Manga3asqSource {
    fn document(&self, url: &str) -> Result<Html> {
        let response = self
            .client
            .get(url)
            .cookies_for(url)
            .rate_limit("manga3asq", REQUEST_LIMIT_MS)
            .max_body_bytes(MAX_DOCUMENT_BYTES)
            .send()?
            .error_for_status()?;
        Ok(Html::parse_document(response.text()?))
    }

    fn chapter_document(&self, item_url: &str) -> Result<Html> {
        let endpoint = format!("{}/ajax/chapters", item_url.trim_end_matches('/'));
        let response = self
            .client
            .post(endpoint)
            .cookies_for(item_url)
            .header("Referer", item_url)
            .header("X-Requested-With", "XMLHttpRequest")
            .rate_limit("manga3asq", REQUEST_LIMIT_MS)
            .max_body_bytes(MAX_CHAPTER_LIST_BYTES)
            .body(Vec::new())
            .send()?
            .error_for_status()?;
        Ok(Html::parse_document(response.text()?))
    }

    fn listing_page(&self, kind: &str, page: u32) -> Result<Paged<CatalogItem>> {
        let sort = match kind {
            "popular" => "views",
            "latest" => "latest",
            other => return Err(Error::new(format!("unknown 3asq listing {other:?}"))),
        };
        parse_catalog(&self.document(&listing_url(sort, page)?)?)
    }
}

impl MangaSource for Manga3asqSource {
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
        if (query.starts_with("https://") || query.starts_with("http://"))
            && self.handle_url(query)?.is_some()
        {
            let entries = self
                .handle_url(query)?
                .and_then(|result| result.item)
                .into_iter()
                .collect();
            return Ok(Paged::new(entries, false));
        }
        parse_catalog(&self.document(&search_url(query, page, filters)?)?)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let slug = item_slug(item.url.as_deref().unwrap_or(&item.key))
            .ok_or_else(|| Error::new("3asq item has no manga slug"))?;
        let url = item_url(&slug);
        parse_details(&self.document(&url)?, &slug, &url)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<MangaChapter>> {
        let slug = item_slug(item.url.as_deref().unwrap_or(&item.key))
            .ok_or_else(|| Error::new("3asq item has no manga slug"))?;
        parse_chapters_at(
            &self.chapter_document(&item_url(&slug))?,
            &slug,
            runtime::now_millis(),
        )
    }

    fn pages(&mut self, item: CatalogItem, chapter: MangaChapter) -> Result<Vec<MangaPage>> {
        let slug = item_slug(item.url.as_deref().unwrap_or(&item.key))
            .ok_or_else(|| Error::new("3asq item has no manga slug"))?;
        let chapter_url =
            canonical_chapter_url(&slug, chapter.url.as_deref().unwrap_or(&chapter.key))
                .ok_or_else(|| Error::new("3asq chapter has no chapter key"))?;
        parse_pages(&self.document(&chapter_url)?, &chapter_url)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(filter_definitions())
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        let slug = item_slug(item.url.as_deref().unwrap_or(&item.key))
            .ok_or_else(|| Error::new("3asq item has no manga slug"))?;
        Ok(Some(item_url(&slug)))
    }

    fn chapter_url(
        &mut self,
        item: &CatalogItem,
        chapter: &MangaChapter,
    ) -> Result<Option<String>> {
        let slug = item_slug(item.url.as_deref().unwrap_or(&item.key))
            .ok_or_else(|| Error::new("3asq item has no manga slug"))?;
        Ok(canonical_chapter_url(
            &slug,
            chapter.url.as_deref().unwrap_or(&chapter.key),
        ))
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let parsed = Url::parse(candidate).map_err(url_error)?;
        if parsed.scheme() != "https" || parsed.host_str() != Some("3asq.online") {
            return Ok(None);
        }
        let Some(slug) = item_slug(parsed.path()) else {
            return Ok(None);
        };
        let item = CatalogItem {
            key: slug.clone(),
            url: Some(item_url(&slug)),
            language: Some(LANGUAGE.to_owned()),
            content_rating: Some(CONTENT_RATING.to_owned()),
            viewer: Some(json!("rtl")),
            ..CatalogItem::default()
        };
        let mut result = UrlResolveResult {
            item: Some(item),
            ..UrlResolveResult::default()
        };
        if let Some(chapter_key) = chapter_key(parsed.path(), &slug) {
            let chapter_url = canonical_chapter_url(&slug, &chapter_key)
                .ok_or_else(|| Error::new("3asq chapter URL is invalid"))?;
            result.chapter_key = Some(chapter_key.clone());
            result.manga_chapter = Some(MangaChapter {
                key: chapter_key.clone(),
                chapter_number: number_in_text(&chapter_key),
                language: Some(LANGUAGE.to_owned()),
                url: Some(chapter_url),
                ..MangaChapter::default()
            });
        }
        Ok(Some(result))
    }
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(
    manatan_sdk::Extension::new().manga("manga3asq", Manga3asqSource::default())
);

fn parse_catalog(document: &Html) -> Result<Paged<CatalogItem>> {
    let card_selector =
        select("div.page-item-detail.manga, .manga__item, div.c-tabs-item__content")?;
    let link_selector = select("div.post-title a:not([target])")?;
    let image_selector = select("img")?;
    let next_selector = select("div.nav-previous a, nav.navigation-ajax, a.nextpostslink")?;
    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();

    for card in document.select(&card_selector) {
        let Some(anchor) = card.select(&link_selector).next() else {
            continue;
        };
        let Some(href) = attr(anchor, "href") else {
            continue;
        };
        let Some(slug) = item_slug(&href) else {
            continue;
        };
        if !seen.insert(slug.clone()) {
            continue;
        }
        let title = element_text(anchor);
        if title.is_empty() {
            seen.remove(&slug);
            continue;
        }
        let cover = card
            .select(&image_selector)
            .find_map(|image| image_url(image))
            .map(|url| absolute_url(BASE_URL, &url))
            .transpose()?
            .map(image_request);
        entries.push(CatalogItem {
            key: slug.clone(),
            title,
            url: Some(item_url(&slug)),
            cover,
            language: Some(LANGUAGE.to_owned()),
            content_rating: Some(CONTENT_RATING.to_owned()),
            viewer: Some(json!("rtl")),
            ..CatalogItem::default()
        });
    }

    Ok(Paged::new(
        entries,
        document.select(&next_selector).next().is_some(),
    ))
}

fn parse_details(document: &Html, slug: &str, url: &str) -> Result<CatalogItem> {
    let title = first_text(
        document,
        "div.post-title h1, div.post-title h3, #manga-title > h1",
    )?
    .ok_or_else(|| Error::new("3asq details page has no title"))?;
    let cover = first_element(document, "div.summary_image img")?
        .and_then(image_url)
        .map(|value| absolute_url(url, &value))
        .transpose()?
        .map(image_request);
    let description = first_text(
        document,
        "div.description-summary div.summary__content, div.summary_content div.manga-excerpt",
    )?
    .filter(|value| !value.is_empty());

    let item_selector = select(".post-content_item")?;
    let heading_selector = select(".summary-heading")?;
    let content_selector = select(".summary-content")?;
    let link_selector = select("a")?;
    let mut authors = Vec::new();
    let mut artists = Vec::new();
    let mut tags = Vec::new();
    let mut status = "unknown";
    let mut alternate_names = None;

    for summary_item in document.select(&item_selector) {
        let heading = summary_item
            .select(&heading_selector)
            .next()
            .map(element_text)
            .unwrap_or_default();
        let Some(content) = summary_item.select(&content_selector).next() else {
            continue;
        };
        let content_text = element_text(content);
        match heading.as_str() {
            "الكاتب" => authors.extend(nonempty_link_text(content, &link_selector)),
            "الرسام" => artists.extend(nonempty_link_text(content, &link_selector)),
            "التصنيفات" => tags.extend(nonempty_link_text(content, &link_selector)),
            "النوع" if !content_text.is_empty() && content_text != "-" => {
                tags.push(content_text)
            }
            "الحالة" => status = parse_status(&content_text),
            "أسماء أخرى" if !content_text.is_empty() => {
                alternate_names = Some(content_text)
            }
            _ => {}
        }
    }
    deduplicate(&mut authors);
    deduplicate(&mut artists);
    deduplicate(&mut tags);
    let description = match (description, alternate_names) {
        (Some(description), Some(names)) => Some(format!("{description}\n\nأسماء أخرى: {names}")),
        (None, Some(names)) => Some(format!("أسماء أخرى: {names}")),
        (description, None) => description,
    };

    Ok(CatalogItem {
        key: slug.to_owned(),
        title,
        url: Some(url.to_owned()),
        cover,
        description,
        authors,
        artists,
        tags,
        status: Some(json!(status)),
        initialized: true,
        language: Some(LANGUAGE.to_owned()),
        content_rating: Some(CONTENT_RATING.to_owned()),
        viewer: Some(json!("rtl")),
        ..CatalogItem::default()
    })
}

fn parse_chapters_at(document: &Html, slug: &str, now_millis: i64) -> Result<Vec<MangaChapter>> {
    let chapter_selector = select("li.wp-manga-chapter")?;
    let anchor_selector = select("a")?;
    let date_selector = select(".chapter-release-date .timediff")?;
    let mut seen = BTreeSet::new();
    let mut chapters = Vec::new();

    for element in document.select(&chapter_selector) {
        let Some(anchor) = element.select(&anchor_selector).next() else {
            continue;
        };
        let Some(href) = attr(anchor, "href") else {
            continue;
        };
        let Some(key) = chapter_key(&href, slug) else {
            continue;
        };
        if !seen.insert(key.clone()) {
            continue;
        }
        let name = element_text(anchor);
        let date_uploaded = element
            .select(&date_selector)
            .next()
            .map(element_text)
            .and_then(|date| parse_chapter_date_at(&date, now_millis));
        chapters.push(MangaChapter {
            key: key.clone(),
            title: (!name.is_empty()).then_some(name.clone()),
            chapter_number: number_in_text(&name).or_else(|| number_in_text(&key)),
            date_uploaded,
            language: Some(LANGUAGE.to_owned()),
            url: canonical_chapter_url(slug, &key),
            source_order: Some(chapters.len() as i32),
            ..MangaChapter::default()
        });
    }
    if chapters.is_empty() {
        return Err(Error::new("3asq chapter endpoint returned no chapters"));
    }
    Ok(chapters)
}

fn parse_pages(document: &Html, chapter_url: &str) -> Result<Vec<MangaPage>> {
    let selector = select(
        "div.reading-content img.wp-manga-chapter-img, div.page-break img.wp-manga-chapter-img",
    )?;
    let mut seen = BTreeSet::new();
    let mut pages = Vec::new();
    for image in document.select(&selector) {
        let Some(candidate) = image_url(image) else {
            continue;
        };
        let url = absolute_url(chapter_url, &candidate)?;
        if !seen.insert(url.clone()) {
            continue;
        }
        let context = page_headers(chapter_url);
        pages.push(MangaPage {
            content: PageContent::Url {
                url,
                context: Some(context.clone()),
            },
            headers: context,
            description: Some(format!("Page {}", pages.len() + 1)),
            ..MangaPage::default()
        });
    }
    if pages.is_empty() {
        return Err(Error::new("3asq chapter page has no reader images"));
    }
    Ok(pages)
}

fn listing_url(sort: &str, page: u32) -> Result<String> {
    let page = page.max(1);
    let path = if page == 1 {
        format!("{BASE_URL}/manga/")
    } else {
        format!("{BASE_URL}/manga/page/{page}/")
    };
    let mut url = Url::parse(&path).map_err(url_error)?;
    url.query_pairs_mut().append_pair("m_orderby", sort);
    Ok(url.to_string())
}

fn search_url(query: &str, page: u32, filters: &Value) -> Result<String> {
    let page = page.max(1);
    let path = if page == 1 {
        format!("{BASE_URL}/")
    } else {
        format!("{BASE_URL}/page/{page}/")
    };
    let mut url = Url::parse(&path).map_err(url_error)?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("s", query.trim());
        pairs.append_pair("post_type", "wp-manga");
        append_selected(&mut pairs, filters, "author", "author");
        append_selected(&mut pairs, filters, "artist", "artist");
        append_selected(&mut pairs, filters, "release", "release");
        append_selected(&mut pairs, filters, "sort", "m_orderby");
        append_selected(&mut pairs, filters, "adult", "adult");
        append_selected(&mut pairs, filters, "genre_mode", "op");
        for status in selected_values(filters, "statuses") {
            pairs.append_pair("status[]", &status);
        }
        for genre in selected_values(filters, "genres") {
            pairs.append_pair("genre[]", &genre);
        }
    }
    Ok(url.to_string())
}

fn append_selected(
    pairs: &mut url::form_urlencoded::Serializer<'_, url::UrlQuery<'_>>,
    filters: &Value,
    filter_id: &str,
    query_name: &str,
) {
    if let Some(value) = selected(filters, filter_id) {
        pairs.append_pair(query_name, value);
    }
}

fn filter_definitions() -> Vec<FilterDefinition> {
    vec![
        FilterDefinition::Text {
            id: "author".to_owned(),
            name: "Author".to_owned(),
            default: String::new(),
        },
        FilterDefinition::Text {
            id: "artist".to_owned(),
            name: "Artist".to_owned(),
            default: String::new(),
        },
        FilterDefinition::Text {
            id: "release".to_owned(),
            name: "Release year".to_owned(),
            default: String::new(),
        },
        select_filter(
            "sort",
            "Sort by",
            &[
                ("Relevance", ""),
                ("Latest", "latest"),
                ("Title", "alphabet"),
                ("Rating", "rating"),
                ("Trending", "trending"),
                ("Views", "views"),
                ("Newest", "new-manga"),
            ],
        ),
        select_filter(
            "adult",
            "Adult content",
            &[("All", ""), ("Exclude", "0"), ("Only", "1")],
        ),
        FilterDefinition::MultiSelect {
            id: "statuses".to_owned(),
            name: "Status".to_owned(),
            options: [
                ("Ongoing", "on-going"),
                ("Completed", "end"),
                ("Cancelled", "canceled"),
                ("On hold", "on-hold"),
                ("Upcoming", "upcoming"),
            ]
            .into_iter()
            .map(|(label, value)| option(label, value))
            .collect(),
            default: Vec::new(),
        },
        FilterDefinition::Separator,
        select_filter(
            "genre_mode",
            "Genre match mode",
            &[("OR", ""), ("AND", "1")],
        ),
        FilterDefinition::MultiSelect {
            id: "genres".to_owned(),
            name: "Genres".to_owned(),
            options: genre_options(),
            default: Vec::new(),
        },
    ]
}

fn genre_options() -> Vec<OptionItem> {
    [
        ("100%", "100"),
        ("3asq", "3asq"),
        ("99%", "99"),
        ("أكشن", "action"),
        ("إيتشي", "ecchi"),
        ("إيسيكاي", "%d8%a5%d9%8a%d8%b3%d9%8a%d9%83%d8%a7%d9%8a"),
        (
            "استراتيجي",
            "%d8%a7%d8%b3%d8%aa%d8%b1%d8%a7%d8%aa%d9%8a%d8%ac%d9%8a",
        ),
        ("اضطهاد", "%d8%a7%d8%b6%d8%b7%d9%87%d8%a7%d8%af"),
        ("العاب", "%d8%a7%d9%84%d8%b9%d8%a7%d8%a8"),
        ("تاريخ", "historical"),
        ("تبادل أجناس", "gender-bender"),
        ("تسلق", "%d8%aa%d8%b3%d9%84%d9%82"),
        ("جريمة", "%d8%ac%d8%b1%d9%8a%d9%85%d8%a9"),
        ("جوائز", "%d8%ac%d9%88%d8%a7%d8%a6%d8%b2"),
        ("جوسي", "josei"),
        ("حرب", "%d8%ad%d8%b1%d8%a8"),
        ("حريم", "harem"),
        (
            "حكايات شعبية",
            "%d8%ad%d9%83%d8%a7%d9%8a%d8%a7%d8%aa-%d8%b4%d8%b9%d8%a8%d9%8a%d8%a9",
        ),
        ("خارق للطبيعة", "supernatural"),
        ("خيال", "fantasy"),
        ("خيال علمي", "sci-fi"),
        ("دراما", "drama"),
        ("رعب", "horror"),
        (
            "رواية خفيفة",
            "%d8%b1%d9%88%d8%a7%d9%8a%d8%a9-%d8%ae%d9%81%d9%8a%d9%81%d8%a9",
        ),
        ("رومانسية", "romance"),
        ("رياضة", "sports"),
        ("ساموراي", "%d8%b3%d8%a7%d9%85%d9%88%d8%b1%d8%a7%d9%8a"),
        ("سينين", "seinen"),
        ("شريحة من الحياة", "slice-of-life"),
        ("شوجو", "shoujo"),
        ("شوغي", "%d8%b4%d9%88%d8%ba%d9%8a"),
        ("شونين", "shounen"),
        ("شياطين", "demons"),
        ("عسكرية", "military"),
        ("علم نفس", "psychological"),
        ("عنف", "%d8%b9%d9%86%d9%81"),
        ("غموض", "mystery"),
        (
            "فضائيِّين",
            "%d9%81%d8%b6%d8%a7%d8%a6%d9%8a%d9%90%d9%91%d9%8a%d9%86",
        ),
        ("فلسفة", "%d9%81%d9%84%d8%b3%d9%81%d8%a9"),
        ("فنون قتالية", "martial-arts"),
        ("قوى خارقة", "super-powers"),
        ("كوميديا", "comedy"),
        ("مأساة", "tragedy"),
        ("مدرسة", "school-life"),
        ("مغامرة", "adventure"),
        ("ميكا", "mecha"),
        ("نفسي", "%d9%86%d9%81%d8%b3%d9%8a"),
        ("نينجا", "%d9%86%d9%8a%d9%86%d8%ac%d8%a7"),
        ("ون شوت", "%d9%88%d9%86-%d8%b4%d9%88%d8%aa"),
        ("ويب-تون", "%d9%88%d9%8a%d8%a8-%d8%aa%d9%88%d9%86"),
    ]
    .into_iter()
    .map(|(label, value)| option(label, value))
    .collect()
}

fn select_filter(id: &str, name: &str, entries: &[(&str, &str)]) -> FilterDefinition {
    FilterDefinition::Select {
        id: id.to_owned(),
        name: name.to_owned(),
        options: entries
            .iter()
            .map(|(label, value)| option(label, value))
            .collect(),
        default_index: 0,
    }
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

fn item_url(slug: &str) -> String {
    format!("{BASE_URL}/manga/{slug}/")
}

fn item_slug(candidate: &str) -> Option<String> {
    let path = candidate_path(candidate);
    let mut segments = path.trim_matches('/').split('/');
    if segments.next()? != "manga" {
        return None;
    }
    let slug = segments.next()?.trim();
    (!slug.is_empty()).then(|| slug.to_owned())
}

fn chapter_key(candidate: &str, slug: &str) -> Option<String> {
    if !candidate.contains('/') {
        let key = candidate.trim();
        return (!key.is_empty() && key != "ajax").then(|| key.to_owned());
    }
    let path = candidate_path(candidate);
    let mut segments = path.trim_matches('/').split('/');
    if segments.next()? != "manga" || segments.next()? != slug {
        return None;
    }
    let key = segments.next()?.trim();
    (!key.is_empty() && key != "ajax" && segments.next().is_none()).then(|| key.to_owned())
}

fn canonical_chapter_url(slug: &str, candidate: &str) -> Option<String> {
    let key = chapter_key(candidate, slug)?;
    Some(format!("{BASE_URL}/manga/{slug}/{key}/"))
}

fn candidate_path(candidate: &str) -> &str {
    let path = candidate
        .split_once("://")
        .and_then(|(_, rest)| rest.find('/').map(|index| &rest[index..]))
        .unwrap_or(candidate);
    path.split(['?', '#']).next().unwrap_or(path)
}

fn parse_status(value: &str) -> &'static str {
    let value = clean_text(value);
    if value.contains("مكتمل") {
        "completed"
    } else if value.contains("مستم") || value.eq_ignore_ascii_case("ongoing") {
        "ongoing"
    } else if value.contains("متوقف") || value.eq_ignore_ascii_case("on hold") {
        "hiatus"
    } else if value.contains("ملغ") || value.eq_ignore_ascii_case("cancelled") {
        "cancelled"
    } else {
        "unknown"
    }
}

fn parse_chapter_date_at(value: &str, now_millis: i64) -> Option<i64> {
    let normalized = normalize_arabic_digits(&clean_text(value));
    if normalized.contains("منذ") {
        let amount = first_integer(&normalized).or_else(|| {
            normalized
                .contains("يومين")
                .then_some(2)
                .or_else(|| normalized.contains("يوم واحد").then_some(1))
        })?;
        let unit_millis = if normalized.contains("دقيق") {
            60_000
        } else if normalized.contains("ساعة") || normalized.contains("ساعات") {
            3_600_000
        } else if normalized.contains("أسبوع") || normalized.contains("اسبوع") {
            7 * 86_400_000
        } else if normalized.contains("شهر") {
            30 * 86_400_000
        } else if normalized.contains("سنة") || normalized.contains("سنوات") {
            365 * 86_400_000
        } else if normalized.contains("يوم") || normalized.contains("أيام") {
            86_400_000
        } else {
            return None;
        };
        return Some(now_millis.saturating_sub(i64::from(amount) * unit_millis));
    }

    let cleaned = normalized.replace('،', " ");
    let parts = cleaned.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 3 {
        return None;
    }
    let day = parts[0].parse::<u32>().ok()?;
    let month = arabic_month(parts[1])?;
    let year = parts[2].parse::<i32>().ok()?;
    NaiveDate::from_ymd_opt(year, month, day)?
        .and_hms_opt(0, 0, 0)
        .map(|date| date.and_utc().timestamp_millis())
}

fn arabic_month(value: &str) -> Option<u32> {
    match value {
        "يناير" => Some(1),
        "فبراير" => Some(2),
        "مارس" => Some(3),
        "أبريل" | "ابريل" => Some(4),
        "مايو" => Some(5),
        "يونيو" => Some(6),
        "يوليو" => Some(7),
        "أغسطس" | "اغسطس" => Some(8),
        "سبتمبر" => Some(9),
        "أكتوبر" | "اكتوبر" => Some(10),
        "نوفمبر" => Some(11),
        "ديسمبر" => Some(12),
        _ => None,
    }
}

fn first_integer(value: &str) -> Option<u32> {
    value
        .split(|character: char| !character.is_ascii_digit())
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn number_in_text(value: &str) -> Option<f32> {
    let normalized = normalize_arabic_digits(value);
    let start = normalized.find(|character: char| character.is_ascii_digit())?;
    normalized[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect::<String>()
        .parse()
        .ok()
}

fn normalize_arabic_digits(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '٠' | '۰' => '0',
            '١' | '۱' => '1',
            '٢' | '۲' => '2',
            '٣' | '۳' => '3',
            '٤' | '۴' => '4',
            '٥' | '۵' => '5',
            '٦' | '۶' => '6',
            '٧' | '۷' => '7',
            '٨' | '۸' => '8',
            '٩' | '۹' => '9',
            other => other,
        })
        .collect()
}

fn page_headers(chapter_url: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("Referer".to_owned(), chapter_url.to_owned()),
        ("User-Agent".to_owned(), BROWSER_USER_AGENT.to_owned()),
    ])
}

fn image_request(url: String) -> ImageRequest {
    ImageRequest::get(url)
        .cookies_for(BASE_URL)
        .header("Referer", REFERER)
        .header("User-Agent", BROWSER_USER_AGENT)
}

fn image_url(image: ElementRef<'_>) -> Option<String> {
    ["data-src", "data-lazy-src", "data-cfsrc", "data-manga-src"]
        .into_iter()
        .find_map(|name| attr(image, name))
        .or_else(|| attr(image, "srcset").and_then(|srcset| best_srcset_candidate(&srcset)))
        .or_else(|| attr(image, "src"))
        .filter(|value| !value.starts_with("data:"))
}

fn best_srcset_candidate(srcset: &str) -> Option<String> {
    srcset
        .split(',')
        .filter_map(|entry| {
            let mut parts = entry.split_whitespace();
            let url = parts.next()?.trim();
            let weight = parts
                .next()
                .and_then(|descriptor| descriptor.trim_end_matches(['w', 'x']).parse::<f32>().ok())
                .unwrap_or_default();
            (!url.is_empty()).then(|| (url.to_owned(), weight))
        })
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(url, _)| url)
}

fn absolute_url(base: &str, candidate: &str) -> Result<String> {
    if candidate.starts_with("//") {
        return Ok(format!("https:{candidate}"));
    }
    Url::parse(base)
        .map_err(url_error)?
        .join(candidate.trim())
        .map(|url| url.to_string())
        .map_err(url_error)
}

fn nonempty_link_text(root: ElementRef<'_>, selector: &Selector) -> Vec<String> {
    root.select(selector)
        .map(element_text)
        .filter(|value| !value.is_empty())
        .collect()
}

fn deduplicate(values: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    values.retain(|value| seen.insert(value.to_lowercase()));
}

fn attr(element: ElementRef<'_>, name: &str) -> Option<String> {
    element
        .value()
        .attr(name)
        .map(clean_text)
        .filter(|value| !value.is_empty())
}

fn first_text(document: &Html, selector: &str) -> Result<Option<String>> {
    let selector = select(selector)?;
    Ok(document
        .select(&selector)
        .map(element_text)
        .find(|value| !value.is_empty()))
}

fn first_element<'a>(document: &'a Html, selector: &str) -> Result<Option<ElementRef<'a>>> {
    let selector = select(selector)?;
    Ok(document.select(&selector).next())
}

fn element_text(element: ElementRef<'_>) -> String {
    clean_text(&element.text().collect::<Vec<_>>().join(" "))
}

fn clean_text(value: &str) -> String {
    value
        .replace('\u{a0}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn select(value: &str) -> Result<Selector> {
    Selector::parse(value)
        .map_err(|error| Error::new(format!("invalid 3asq selector {value:?}: {error}")))
}

fn url_error(error: impl ToString) -> Error {
    Error::new(format!("3asq URL error: {}", error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    const CATALOG: &str = include_str!("../fixtures/catalog.html");
    const SEARCH: &str = include_str!("../fixtures/search.html");
    const DETAILS: &str = include_str!("../fixtures/details.html");
    const CHAPTERS: &str = include_str!("../fixtures/chapters.html");
    const CHAPTER: &str = include_str!("../fixtures/chapter.html");
    const MANIFEST: &str = include_str!("../manifest.json");
    const ICON: &[u8] = include_bytes!("../assets/icon.png");
    const ICON_SHA256: &str = "e165993e8c6cb99b72b6e450aa2e9872b9ac9c02e0839288404f318389b5ef40";

    #[test]
    fn parses_current_catalog_and_ignores_external_team_links() {
        let page = parse_catalog(&Html::parse_document(CATALOG)).expect("catalog parses");
        assert_eq!(page.entries.len(), 2);
        assert!(page.has_next_page);
        assert_eq!(page.entries[0].key, "kingdom-2");
        assert_eq!(page.entries[0].title, "Kingdom (WAN)");
        assert_eq!(
            page.entries[0]
                .cover
                .as_ref()
                .map(|cover| cover.url.as_str()),
            Some("https://3asq.online/wp-content/uploads/2024/03/cover.jpg")
        );
        assert_eq!(
            page.entries[1]
                .cover
                .as_ref()
                .map(|cover| cover.url.as_str()),
            Some("https://3asq.online/wp-content/uploads/2019/04/v1-1-175x238.jpg")
        );
    }

    #[test]
    fn parses_current_search_result_rows() {
        let page = parse_catalog(&Html::parse_document(SEARCH)).expect("search results parse");
        assert_eq!(page.entries.len(), 2);
        assert!(!page.has_next_page);
        assert_eq!(page.entries[0].key, "kingdom-hearts");
        assert_eq!(page.entries[0].title, "Kingdom Hearts");
        assert_eq!(
            page.entries[0]
                .cover
                .as_ref()
                .map(|cover| cover.url.as_str()),
            Some(
                "https://3asq.online/wp-content/uploads/2024/10/KINGDOM-HEARTS-NEW-COVER-193x278.jpg"
            )
        );
        assert_eq!(page.entries[1].key, "kingdom-2");
    }

    #[test]
    fn parses_details_metadata_and_arabic_status() {
        let item = parse_details(
            &Html::parse_document(DETAILS),
            "kingdom-2",
            &item_url("kingdom-2"),
        )
        .expect("details parse");
        assert_eq!(item.title, "Kingdom (WAN)");
        assert_eq!(item.authors, vec!["ياسوهيسا هارا"]);
        assert_eq!(item.artists, vec!["ياسوهيسا هارا"]);
        assert_eq!(item.tags, vec!["أكشن", "تاريخ", "دراما", "مانجا"]);
        assert_eq!(item.status, Some(json!("ongoing")));
        assert!(item
            .description
            .as_deref()
            .is_some_and(|value| value.contains("أسماء أخرى")));
        assert!(item.initialized);
    }

    #[test]
    fn parses_ajax_chapters_and_arabic_dates() {
        let now = 1_785_888_000_000_i64;
        let chapters =
            parse_chapters_at(&Html::parse_document(CHAPTERS), "kingdom-2", now).unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].key, "884");
        assert_eq!(chapters[0].chapter_number, Some(884.0));
        assert_eq!(chapters[0].date_uploaded, Some(now - 2 * 86_400_000));
        assert_eq!(chapters[1].date_uploaded, Some(1_784_246_400_000));
        assert_eq!(
            chapters[1].url.as_deref(),
            Some("https://3asq.online/manga/kingdom-2/883/")
        );
    }

    #[test]
    fn parses_only_reader_images_with_required_context() {
        let chapter_url = "https://3asq.online/manga/kingdom-2/884/";
        let pages = parse_pages(&Html::parse_document(CHAPTER), chapter_url).unwrap();
        assert_eq!(pages.len(), 3);
        let PageContent::Url { url, context } = &pages[2].content else {
            panic!("expected URL page");
        };
        assert_eq!(
            url,
            "https://3asq.online/wp-content/uploads/WP-manga/data/book/chapter/03.jpg"
        );
        assert_eq!(
            context.as_ref().and_then(|headers| headers.get("Referer")),
            Some(&chapter_url.to_owned())
        );
        assert_eq!(
            context
                .as_ref()
                .and_then(|headers| headers.get("User-Agent"))
                .map(String::as_str),
            Some(BROWSER_USER_AGENT)
        );
    }

    #[test]
    fn builds_paginated_search_with_all_filter_shapes() {
        let url = search_url(
            "kingdom",
            2,
            &json!({
                "author": "هارا",
                "artist": "هارا",
                "release": "2006",
                "sort": "views",
                "adult": "0",
                "statuses": ["on-going", "on-hold"],
                "genre_mode": "1",
                "genres": ["historical", "drama"]
            }),
        )
        .unwrap();
        let parsed = Url::parse(&url).unwrap();
        assert_eq!(parsed.path(), "/page/2/");
        let pairs = parsed
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        assert!(pairs.contains(&("s".to_owned(), "kingdom".to_owned())));
        assert!(pairs.contains(&("status[]".to_owned(), "on-going".to_owned())));
        assert!(pairs.contains(&("status[]".to_owned(), "on-hold".to_owned())));
        assert!(pairs.contains(&("genre[]".to_owned(), "historical".to_owned())));
        assert!(pairs.contains(&("genre[]".to_owned(), "drama".to_owned())));
    }

    #[test]
    fn resolves_item_and_chapter_urls() {
        let mut source = Manga3asqSource::default();
        let item = source
            .handle_url("https://3asq.online/manga/kingdom-2/")
            .unwrap()
            .unwrap();
        assert_eq!(item.item.unwrap().key, "kingdom-2");
        assert!(item.chapter_key.is_none());

        let chapter = source
            .handle_url("https://3asq.online/manga/kingdom-2/884/")
            .unwrap()
            .unwrap();
        assert_eq!(chapter.chapter_key.as_deref(), Some("884"));
        assert_eq!(chapter.manga_chapter.unwrap().chapter_number, Some(884.0));
        assert!(source
            .handle_url("https://example.com/manga/kingdom-2/")
            .unwrap()
            .is_none());
    }

    #[test]
    fn exposes_current_upstream_filter_set() {
        let filters = filter_definitions();
        assert_eq!(filters.len(), 9);
        let FilterDefinition::MultiSelect { options, .. } = &filters[5] else {
            panic!("status filter must be a multi-select");
        };
        assert_eq!(options.len(), 5);
        let FilterDefinition::MultiSelect { options, .. } = &filters[8] else {
            panic!("genre filter must be a multi-select");
        };
        assert_eq!(options.len(), 50);
        assert!(options
            .iter()
            .any(|option| option.label == "فنون قتالية" && option.value == "martial-arts"));
    }

    #[test]
    fn metadata_and_icon_match_the_package() {
        let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest parses");
        assert_eq!(manifest["id"], "manga3asq");
        assert_eq!(manifest["contentType"], "manga");
        assert_eq!(manifest["license"], "Apache-2.0");
        assert_eq!(manifest["sources"][0]["id"], "manga3asq");
        assert_eq!(manifest["sources"][0]["lang"], "ar");
        assert_eq!(
            manifest["permissions"]["network"]["allow"],
            json!(["https://3asq.online"])
        );
        assert_eq!(format!("{:x}", Sha256::digest(ICON)), ICON_SHA256);
        assert_eq!(manifest["assets"][0]["sha256"], ICON_SHA256);
    }
}
