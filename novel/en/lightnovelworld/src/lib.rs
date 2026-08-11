use chrono::DateTime;
use manatan_common::{normalize_space, require};
use manatan_sdk::{
    client::Client,
    model::{
        CatalogItem, FilterDefinition, ImageRequest, ImageRequestContext, NovelChapter,
        NovelChapterPage, NovelContentBlock, NovelText, OptionItem, Paged, UrlResolveResult,
    },
    Error, NovelSource, Result,
};
use serde_json::{json, Value};
use url::Url;

#[cfg(target_arch = "wasm32")]
const SOURCE_ID: &str = "lightnovelworld";
const BASE_URL: &str = "https://chikari.moe";
const LEGACY_BASE_URL: &str = "https://lightnovelworld.org";
const PAGE_SIZE: usize = 36;
const CHAPTER_PAGE_SIZE: u64 = 200;
const MAX_JSON_BYTES: u64 = 16_000_000;
const REQUEST_LIMIT_MS: u32 = 150;

pub struct LightNovelWorldSource {
    client: Client,
}

impl Default for LightNovelWorldSource {
    fn default() -> Self {
        Self {
            client: Client::browser().cookies_for(BASE_URL),
        }
    }
}

impl LightNovelWorldSource {
    fn get_json(&self, url: &str) -> Result<Value> {
        self.client
            .get(url)
            .cookies_for(BASE_URL)
            .rate_limit("chikari", REQUEST_LIMIT_MS)
            .max_body_bytes(MAX_JSON_BYTES)
            .send()?
            .error_for_status()?
            .json()
    }

    fn catalog_page(
        &self,
        sort: &str,
        query: &str,
        page: u32,
        filters: &Value,
    ) -> Result<Paged<CatalogItem>> {
        let page = page.max(1);
        let offset = (usize::try_from(page)
            .unwrap_or(usize::MAX)
            .saturating_sub(1))
        .saturating_mul(PAGE_SIZE);
        let mut url = Url::parse(&format!("{BASE_URL}/api/novels"))
            .map_err(|error| Error::new(error.to_string()))?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("sort", sort);
            pairs.append_pair("limit", &PAGE_SIZE.to_string());
            pairs.append_pair("offset", &offset.to_string());
            if !query.trim().is_empty() {
                pairs.append_pair("q", query.trim());
            }
            if filter_bool(filters, "adult") {
                pairs.append_pair("adult", "true");
            }
            append_values(&mut pairs, filters, "genres", "genre");
            append_values(&mut pairs, filters, "languages", "language");
            append_values(&mut pairs, filters, "statuses", "status");
            append_values(&mut pairs, filters, "years", "year");
            if let Some(value) = filter_string(filters, "min_chapters") {
                pairs.append_pair("min_chapters", value);
            }
        }
        let response = self.get_json(url.as_str())?;
        let entries = parse_catalog_items(
            response
                .get("items")
                .and_then(Value::as_array)
                .ok_or_else(|| Error::new("Chikari catalog response has no items"))?,
        )?;
        let total = response
            .get("total")
            .and_then(Value::as_u64)
            .unwrap_or((offset + entries.len()) as u64);
        Ok(Paged::new(
            entries,
            (offset as u64).saturating_add(PAGE_SIZE as u64) < total,
        ))
    }

    fn listing_page(&self, listing: &str, page: u32) -> Result<Paged<CatalogItem>> {
        let sort = match listing {
            "popular" => "popular",
            "latest" => "updated",
            other => return Err(Error::new(format!("unknown Chikari listing {other:?}"))),
        };
        self.catalog_page(sort, "", page, &json!({}))
    }

    fn item_url(item: &CatalogItem) -> Result<String> {
        let slug = item_slug(item.url.as_deref().unwrap_or(&item.key))
            .or_else(|| item_slug(&item.key))
            .ok_or_else(|| Error::new("Chikari item has no novel slug"))?;
        Ok(canonical_item_url(&slug))
    }

    fn slug(item: &CatalogItem) -> Result<String> {
        item_slug(item.url.as_deref().unwrap_or(&item.key))
            .or_else(|| item_slug(&item.key))
            .ok_or_else(|| Error::new("Chikari item has no novel slug"))
    }

    fn chapters_value(&self, slug: &str, offset: u64, limit: u64) -> Result<Value> {
        let mut url = Url::parse(&format!("{BASE_URL}/api/novels/{slug}/chapters"))
            .map_err(|error| Error::new(error.to_string()))?;
        url.query_pairs_mut()
            .append_pair("order", "asc")
            .append_pair("limit", &limit.to_string())
            .append_pair("offset", &offset.to_string());
        self.get_json(url.as_str())
    }

    fn genres(&self) -> Result<Vec<OptionItem>> {
        let value = self.get_json(&format!("{BASE_URL}/api/novels/genres"))?;
        parse_genres(&value)
    }
}

impl NovelSource for LightNovelWorldSource {
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
        let sort = filter_string(filters, "sort").unwrap_or("trending");
        self.catalog_page(sort, query, page, filters)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let slug = Self::slug(&item)?;
        parse_details(
            &self.get_json(&format!("{BASE_URL}/api/novels/{slug}"))?,
            &slug,
        )
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        let slug = Self::slug(&item)?;
        let mut chapters = Vec::new();
        let mut offset = 0u64;
        loop {
            let response = self.chapters_value(&slug, offset, CHAPTER_PAGE_SIZE)?;
            let values = response
                .get("items")
                .and_then(Value::as_array)
                .ok_or_else(|| Error::new("Chikari chapter response has no items"))?;
            let page = parse_chapters(values, &slug, offset)?;
            let received = page.len() as u64;
            chapters.extend(page);
            let total = response
                .get("total")
                .and_then(Value::as_u64)
                .unwrap_or(offset + received);
            offset = offset.saturating_add(received);
            if received == 0 || offset >= total {
                break;
            }
        }
        require(
            (!chapters.is_empty()).then_some(()),
            "Chikari novel has no chapters",
        )?;
        Ok(chapters)
    }

    fn chapters_page(&mut self, item: CatalogItem, page: u32) -> Result<NovelChapterPage> {
        let slug = Self::slug(&item)?;
        let page = page.max(1);
        let offset = u64::from(page - 1) * CHAPTER_PAGE_SIZE;
        let response = self.chapters_value(&slug, offset, CHAPTER_PAGE_SIZE)?;
        let entries = parse_chapters(
            response
                .get("items")
                .and_then(Value::as_array)
                .ok_or_else(|| Error::new("Chikari chapter response has no items"))?,
            &slug,
            offset,
        )?;
        let total = response
            .get("total")
            .and_then(Value::as_u64)
            .unwrap_or(offset + entries.len() as u64);
        Ok(NovelChapterPage {
            has_next_page: offset.saturating_add(entries.len() as u64) < total,
            page_count: Some(
                total
                    .div_ceil(CHAPTER_PAGE_SIZE)
                    .max(u64::from(page))
                    .min(u64::from(u32::MAX)) as u32,
            ),
            entries,
        })
    }

    fn text(&mut self, item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let slug = Self::slug(&item)?;
        let number = chapter_number(&chapter)
            .ok_or_else(|| Error::new("Chikari chapter has no chapter number"))?;
        let token = chapter_token(number);
        let chapter_url = canonical_chapter_url(&slug, &token);
        parse_text(
            &self.get_json(&format!(
                "{BASE_URL}/api/novels/{slug}/chapters/{token}/read"
            ))?,
            &chapter_url,
        )
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(filter_definitions(self.genres()?))
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        Ok(Some(Self::item_url(item)?))
    }

    fn chapter_url(
        &mut self,
        item: &CatalogItem,
        chapter: &NovelChapter,
    ) -> Result<Option<String>> {
        let slug = Self::slug(item)?;
        let number = chapter_number(chapter)
            .ok_or_else(|| Error::new("Chikari chapter has no chapter number"))?;
        Ok(Some(canonical_chapter_url(&slug, &chapter_token(number))))
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let Some(location) = parse_location(candidate) else {
            return Ok(None);
        };
        let item_url = canonical_item_url(&location.slug);
        let mut item = CatalogItem::new(legacy_item_key(&location.slug), "");
        item.url = Some(item_url);
        item.language = Some("en".into());
        let novel_chapter = location.chapter.map(|token| NovelChapter {
            key: legacy_chapter_key(&location.slug, &token),
            chapter_number: token.parse::<f32>().ok(),
            url: Some(canonical_chapter_url(&location.slug, &token)),
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

struct Location {
    slug: String,
    chapter: Option<String>,
}

fn parse_location(candidate: &str) -> Option<Location> {
    let url = Url::parse(candidate).ok()?;
    let host = url.host_str()?.trim_start_matches("www.");
    if host != "chikari.moe" && host != "lightnovelworld.org" {
        return None;
    }
    let segments = url
        .path_segments()?
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    match segments.as_slice() {
        ["novels", slug] => Some(Location {
            slug: (*slug).to_owned(),
            chapter: None,
        }),
        ["novels", slug, chapter] if valid_chapter_token(chapter) => Some(Location {
            slug: (*slug).to_owned(),
            chapter: Some((*chapter).to_owned()),
        }),
        ["novel", slug] => Some(Location {
            slug: (*slug).to_owned(),
            chapter: None,
        }),
        ["novel", slug, "chapter", chapter] if valid_chapter_token(chapter) => Some(Location {
            slug: (*slug).to_owned(),
            chapter: Some((*chapter).to_owned()),
        }),
        _ => None,
    }
}

fn item_slug(candidate: &str) -> Option<String> {
    parse_location(candidate)
        .map(|location| location.slug)
        .or_else(|| {
            let slug = candidate.trim_matches('/');
            (!slug.is_empty() && !slug.contains('/')).then(|| slug.to_owned())
        })
}

fn valid_chapter_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_digit() || character == '.')
        && value.parse::<f64>().is_ok_and(|number| number.is_finite())
}

fn canonical_item_url(slug: &str) -> String {
    format!("{BASE_URL}/novels/{slug}")
}

fn canonical_chapter_url(slug: &str, chapter: &str) -> String {
    format!("{BASE_URL}/novels/{slug}/{chapter}")
}

fn legacy_item_key(slug: &str) -> String {
    format!("{LEGACY_BASE_URL}/novel/{slug}/")
}

fn legacy_chapter_key(slug: &str, chapter: &str) -> String {
    format!("{LEGACY_BASE_URL}/novel/{slug}/chapter/{chapter}/")
}

fn chapter_token(number: f64) -> String {
    if number.fract() == 0.0 {
        format!("{number:.0}")
    } else {
        number.to_string()
    }
}

fn chapter_number(chapter: &NovelChapter) -> Option<f64> {
    chapter.chapter_number.map(f64::from).or_else(|| {
        chapter
            .url
            .as_deref()
            .or(Some(chapter.key.as_str()))
            .and_then(parse_location)
            .and_then(|location| location.chapter)
            .and_then(|value| value.parse().ok())
    })
}

fn parse_catalog_items(values: &[Value]) -> Result<Vec<CatalogItem>> {
    values
        .iter()
        .map(|value| {
            let slug = required_string(value, "slug", "catalog item")?;
            let title = required_string(value, "title", "catalog item")?;
            let page_url = canonical_item_url(slug);
            let mut item = CatalogItem::new(legacy_item_key(slug), title);
            item.url = Some(page_url.clone());
            item.cover = value
                .get("cover_url")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(|cover| image(cover, &page_url));
            item.status = value
                .get("status")
                .and_then(Value::as_str)
                .map(|value| json!(normalize_status(value)));
            item.rating = value
                .get("rating")
                .and_then(Value::as_f64)
                .map(|value| value as f32);
            item.language = Some("en".into());
            item.content_rating = Some(
                if value
                    .get("is_nsfw")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    "adult"
                } else {
                    "suggestive"
                }
                .into(),
            );
            Ok(item)
        })
        .collect()
}

fn parse_details(value: &Value, slug: &str) -> Result<CatalogItem> {
    let title = required_string(value, "title", "novel")?;
    let page_url = canonical_item_url(slug);
    let mut item = CatalogItem::new(legacy_item_key(slug), title);
    item.url = Some(page_url.clone());
    item.description = value
        .get("description")
        .and_then(Value::as_str)
        .map(normalize_space)
        .filter(|value| !value.is_empty());
    item.authors = value
        .get("authors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|author| author.get("name").and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect();
    item.tags = value
        .get("genres")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(
            value
                .get("tags")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|tag| {
                    !tag.get("is_spoiler")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                }),
        )
        .filter_map(|tag| tag.get("name").and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect();
    item.cover = value
        .get("cover_url")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(|cover| image(cover, &page_url));
    item.status = value
        .get("status")
        .and_then(Value::as_str)
        .map(|value| json!(normalize_status(value)));
    item.rating = value
        .get("rating")
        .and_then(Value::as_f64)
        .map(|value| value as f32);
    item.initialized = true;
    item.language = Some("en".into());
    item.content_rating = Some(
        if value
            .get("is_nsfw")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            "adult"
        } else {
            "suggestive"
        }
        .into(),
    );
    Ok(item)
}

fn parse_chapters(values: &[Value], slug: &str, offset: u64) -> Result<Vec<NovelChapter>> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let number = value
                .get("number")
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite())
                .ok_or_else(|| Error::new("Chikari chapter has no number"))?;
            let token = chapter_token(number);
            Ok(NovelChapter {
                key: legacy_chapter_key(slug, &token),
                title: value
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .or_else(|| Some(format!("Chapter {token}"))),
                chapter_number: Some(number as f32),
                volume_number: value
                    .get("volume")
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse().ok()),
                date_uploaded: value
                    .get("created_at")
                    .and_then(Value::as_str)
                    .and_then(parse_date),
                url: Some(canonical_chapter_url(slug, &token)),
                language: Some(
                    value
                        .get("lang")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .unwrap_or("en")
                        .to_owned(),
                ),
                source_order: Some(offset.saturating_add(index as u64).min(i32::MAX as u64) as i32),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(value: &Value, chapter_url: &str) -> Result<NovelText> {
    if value
        .get("locked")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(Error::new(
            "This Chikari chapter is temporarily locked for early access.",
        ));
    }
    let body = value
        .get("body")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::new("Chikari chapter has no readable text"))?;
    let rendered = body_to_html(body);
    Ok(NovelText {
        html: Some(rendered),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        base_url: Some(chapter_url.into()),
        image_context: Some(ImageRequestContext {
            headers: [("Referer".into(), chapter_url.into())]
                .into_iter()
                .collect(),
            cookie_url: Some(BASE_URL.into()),
        }),
        blocks: vec![NovelContentBlock::Text {
            text: body.to_owned(),
            html: false,
        }],
        ..NovelText::default()
    })
}

fn body_to_html(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| format!("<p>{}</p>", escape_html(line)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn parse_date(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.timestamp_millis())
}

fn parse_genres(value: &Value) -> Result<Vec<OptionItem>> {
    value
        .as_array()
        .ok_or_else(|| Error::new("Chikari genre response is not an array"))?
        .iter()
        .map(|genre| {
            Ok(OptionItem {
                label: required_string(genre, "name", "genre")?.to_owned(),
                value: required_string(genre, "slug", "genre")?.to_owned(),
            })
        })
        .collect()
}

fn filter_definitions(genres: Vec<OptionItem>) -> Vec<FilterDefinition> {
    vec![
        select_filter(
            "sort",
            "Sort by",
            &[
                ("Trending", "trending"),
                ("Popularity", "popular"),
                ("Top rated", "top_rated"),
                ("Recently updated", "updated"),
                ("Recently added", "added"),
                ("Most bookmarked", "most_bookmarked"),
                ("Random", "random"),
            ],
        ),
        FilterDefinition::CheckBox {
            id: "adult".into(),
            name: "Include 18+ titles".into(),
            default: false,
        },
        FilterDefinition::MultiSelect {
            id: "statuses".into(),
            name: "Status".into(),
            options: [
                ("Releasing", "releasing"),
                ("Completed", "completed"),
                ("Hiatus", "hiatus"),
                ("Cancelled", "cancelled"),
            ]
            .into_iter()
            .map(|(label, value)| option(label, value))
            .collect(),
            default: Vec::new(),
        },
        FilterDefinition::MultiSelect {
            id: "languages".into(),
            name: "Original language".into(),
            options: [
                ("Japanese", "ja"),
                ("Korean", "ko"),
                ("Chinese", "zh"),
                ("English", "en"),
                ("Other", "other"),
            ]
            .into_iter()
            .map(|(label, value)| option(label, value))
            .collect(),
            default: Vec::new(),
        },
        FilterDefinition::MultiSelect {
            id: "genres".into(),
            name: "Genres".into(),
            options: genres,
            default: Vec::new(),
        },
        FilterDefinition::Text {
            id: "years".into(),
            name: "Year".into(),
            default: String::new(),
        },
        FilterDefinition::Text {
            id: "min_chapters".into(),
            name: "Minimum chapters".into(),
            default: String::new(),
        },
    ]
}

fn select_filter(id: &str, name: &str, values: &[(&str, &str)]) -> FilterDefinition {
    FilterDefinition::Select {
        id: id.into(),
        name: name.into(),
        options: values
            .iter()
            .map(|(label, value)| option(label, value))
            .collect(),
        default_index: 0,
    }
}

fn option(label: &str, value: &str) -> OptionItem {
    OptionItem {
        label: label.into(),
        value: value.into(),
    }
}

fn append_values(
    pairs: &mut url::form_urlencoded::Serializer<'_, url::UrlQuery<'_>>,
    filters: &Value,
    filter_id: &str,
    query_name: &str,
) {
    for value in filter_values(filters, filter_id) {
        pairs.append_pair(query_name, &value);
    }
}

fn filter_values(filters: &Value, key: &str) -> Vec<String> {
    match filters.get(key) {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect(),
        Some(Value::String(value)) if !value.is_empty() => vec![value.clone()],
        _ => Vec::new(),
    }
}

fn filter_string<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn filter_bool(filters: &Value, key: &str) -> bool {
    filters.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn required_string<'a>(value: &'a Value, key: &str, kind: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::new(format!("Chikari {kind} has no {key}")))
}

fn image(url: &str, referer: &str) -> ImageRequest {
    ImageRequest::get(url)
        .header("Referer", referer)
        .cookies_for(BASE_URL)
}

fn normalize_status(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "releasing" | "ongoing" => "ongoing",
        "completed" | "complete" => "completed",
        "hiatus" | "on hiatus" => "hiatus",
        "cancelled" | "canceled" => "cancelled",
        _ => "unknown",
    }
}

#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel(SOURCE_ID, LightNovelWorldSource::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOG: &str = include_str!("../tests/fixtures/catalog.json");
    const DETAILS: &str = include_str!("../tests/fixtures/details.json");
    const CHAPTERS: &str = include_str!("../tests/fixtures/chapters.json");
    const CHAPTER: &str = include_str!("../tests/fixtures/chapter.json");
    const GENRES: &str = include_str!("../tests/fixtures/genres.json");

    #[test]
    fn parses_catalog_and_preserves_legacy_item_keys() {
        let value: Value = serde_json::from_str(CATALOG).unwrap();
        let items = parse_catalog_items(value["items"].as_array().unwrap()).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Shadow Slave");
        assert_eq!(
            items[0].key,
            "https://lightnovelworld.org/novel/shadow-slave/"
        );
        assert_eq!(
            items[0].url.as_deref(),
            Some("https://chikari.moe/novels/shadow-slave")
        );
        assert_eq!(items[0].status, Some(json!("ongoing")));
        assert_eq!(items[1].content_rating.as_deref(), Some("adult"));
    }

    #[test]
    fn parses_details_metadata() {
        let value: Value = serde_json::from_str(DETAILS).unwrap();
        let item = parse_details(&value, "shadow-slave").unwrap();
        assert_eq!(item.authors, vec!["GuiltyThree"]);
        assert!(item.tags.contains(&"Action".to_owned()));
        assert!(item.tags.contains(&"Magic".to_owned()));
        assert!(!item.tags.contains(&"Spoiler".to_owned()));
        assert!(item.initialized);
        assert_eq!(item.rating, Some(9.5));
    }

    #[test]
    fn parses_paginated_chapters_with_legacy_keys() {
        let value: Value = serde_json::from_str(CHAPTERS).unwrap();
        let chapters =
            parse_chapters(value["items"].as_array().unwrap(), "shadow-slave", 200).unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].chapter_number, Some(201.0));
        assert_eq!(chapters[0].source_order, Some(200));
        assert_eq!(
            chapters[0].key,
            "https://lightnovelworld.org/novel/shadow-slave/chapter/201/"
        );
        assert_eq!(
            chapters[0].url.as_deref(),
            Some("https://chikari.moe/novels/shadow-slave/201")
        );
        assert!(chapters[0].date_uploaded.is_some());
    }

    #[test]
    fn renders_public_chapter_text_safely() {
        let value: Value = serde_json::from_str(CHAPTER).unwrap();
        let text = parse_text(&value, "https://chikari.moe/novels/shadow-slave/1").unwrap();
        let html = text.html.unwrap();
        assert!(html.contains("<p>A frail-looking reader &amp; survivor.</p>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(matches!(
            &text.blocks[0],
            NovelContentBlock::Text { html: false, .. }
        ));
    }

    #[test]
    fn rejects_locked_chapter_without_hiding_the_reason() {
        let error = parse_text(
            &json!({"locked": true, "protected_window": 5}),
            "https://chikari.moe/novels/example/10",
        )
        .unwrap_err();
        assert!(error.to_string().contains("early access"));
    }

    #[test]
    fn migrates_old_and_new_urls_to_canonical_chikari_urls() {
        let mut source = LightNovelWorldSource::default();
        for candidate in [
            "https://lightnovelworld.org/novel/shadow-slave/chapter/12/",
            "https://chikari.moe/novels/shadow-slave/12",
        ] {
            let result = source.handle_url(candidate).unwrap().unwrap();
            let item = result.item.unwrap();
            assert_eq!(
                item.url.as_deref(),
                Some("https://chikari.moe/novels/shadow-slave")
            );
            let chapter = result.novel_chapter.unwrap();
            assert_eq!(chapter.chapter_number, Some(12.0));
            assert_eq!(
                chapter.url.as_deref(),
                Some("https://chikari.moe/novels/shadow-slave/12")
            );
        }
    }

    #[test]
    fn parses_live_filter_genres() {
        let value: Value = serde_json::from_str(GENRES).unwrap();
        let genres = parse_genres(&value).unwrap();
        assert_eq!(genres.len(), 3);
        assert_eq!(genres[0], option("Action", "action"));
        let filters = filter_definitions(genres);
        assert_eq!(filters.len(), 7);
    }

    #[test]
    fn constants_and_image_context_use_only_the_new_hosts() {
        assert_eq!(BASE_URL, "https://chikari.moe");
        let request = image(
            "https://cdn.chikari.moe/novels/53/cover.webp",
            "https://chikari.moe/novels/shadow-slave",
        );
        assert_eq!(request.cookie_url.as_deref(), Some(BASE_URL));
        assert_eq!(
            request.headers.get("Referer").map(String::as_str),
            Some("https://chikari.moe/novels/shadow-slave")
        );
    }
}
