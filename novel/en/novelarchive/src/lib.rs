use manatan_common::{absolute_url, require};
use manatan_sdk::{
    client::Client,
    model::{
        CatalogItem, FilterDefinition, ImageRequest, NovelChapter, NovelChapterPage,
        NovelContentBlock, NovelText, OptionItem, Paged, UrlResolveResult,
    },
    Error, NovelSource, Result,
};
use serde_json::{json, Value};
use url::Url;

#[cfg(target_arch = "wasm32")]
const SOURCE_ID: &str = "novelarchive";
const BASE_URL: &str = "https://novelarchive.cc";
const API_URL: &str = "https://novelarchive.cc/api";
const PAGE_SIZE: u32 = 24;
const CHAPTER_PAGE_SIZE: usize = 200;
const EXCLUDED_GENRES: &str = "Adult,Erotica,Smut,Explicit Sex,Ecchi";

pub struct NovelArchiveSource {
    client: Client,
}

impl Default for NovelArchiveSource {
    fn default() -> Self {
        Self {
            client: Client::browser(),
        }
    }
}

impl NovelArchiveSource {
    fn get_json(&self, url: &str) -> Result<Value> {
        self.client.get(url).send()?.error_for_status()?.json()
    }

    fn browse(
        &self,
        page: u32,
        query: Option<&str>,
        filters: &Value,
    ) -> Result<Paged<CatalogItem>> {
        let mut url = Url::parse(&format!("{API_URL}/novels"))
            .map_err(|error| Error::new(error.to_string()))?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("page", &page.max(1).to_string());
            pairs.append_pair("per_page", &PAGE_SIZE.to_string());
            pairs.append_pair("genres_exclude", EXCLUDED_GENRES);
            pairs.append_pair(
                "sort",
                filters
                    .get("sort")
                    .and_then(Value::as_str)
                    .unwrap_or("popular"),
            );
            if let Some(query) = query.filter(|value| !value.trim().is_empty()) {
                pairs.append_pair("search", query.trim());
                pairs.append_pair("fuzzy", "1");
            }
            if let Some(status) = filters
                .get("status")
                .and_then(Value::as_str)
                .filter(|value| *value != "all")
            {
                pairs.append_pair("status", status);
            }
            if let Some(genre) = filters
                .get("genre")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                pairs.append_pair("genres_include", genre);
            }
        }
        let response = self.get_json(url.as_str())?;
        let values = response
            .get("novels")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::new("Novel Archive response has no novels"))?;
        let items = values
            .iter()
            .filter(|value| !is_restricted(value))
            .map(Self::parse_item)
            .collect::<Result<Vec<_>>>()?;
        let has_next = response
            .pointer("/pagination/has_next")
            .and_then(Value::as_bool)
            .unwrap_or(items.len() as u32 == PAGE_SIZE);
        Ok(Paged::new(items, has_next))
    }

    fn parse_item(value: &Value) -> Result<CatalogItem> {
        let id =
            string_value(value, "id").ok_or_else(|| Error::new("Novel Archive novel has no id"))?;
        let title = string_value(value, "title")
            .ok_or_else(|| Error::new("Novel Archive novel has no title"))?;
        let page_url = format!("{BASE_URL}/novel?id={id}");
        let mut item = CatalogItem::new(id.clone(), title);
        item.url = Some(page_url.clone());
        item.description = string_value(value, "description");
        item.authors = string_value(value, "author")
            .filter(|author| !author.eq_ignore_ascii_case("unknown"))
            .into_iter()
            .collect();
        item.tags = genres(value);
        item.cover = ["cover_url", "image_url", "novel_image"]
            .iter()
            .find_map(|key| string_value(value, key))
            .map(|cover| absolute_url(BASE_URL, &cover))
            .transpose()?
            .map(|cover| ImageRequest::get(cover).header("Referer", &page_url));
        item.status = string_value(value, "release_status")
            .or_else(|| string_value(value, "ongoing"))
            .map(|status| json!(normalize_status(&status)));
        item.initialized = value.get("description").is_some();
        item.language = Some("en".into());
        item.content_rating = Some("suggestive".into());
        item.extra.insert("novelArchiveId".into(), json!(id));
        if let Some(count) = number_value(value, "total_chapters") {
            item.extra.insert("chapterCount".into(), json!(count));
        }
        Ok(item)
    }

    fn id(item: &CatalogItem) -> Result<String> {
        if let Some(id) = item
            .extra
            .get("novelArchiveId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            return Ok(id.to_owned());
        }
        if !item.key.starts_with("http") && !item.key.is_empty() {
            return Ok(item.key.clone());
        }
        let candidate = item.url.as_deref().unwrap_or(&item.key);
        Url::parse(candidate)
            .map_err(|error| Error::new(error.to_string()))?
            .query_pairs()
            .find_map(|(key, value)| (key == "id").then(|| value.into_owned()))
            .ok_or_else(|| Error::new("Novel Archive URL has no novel id"))
    }

    fn details_value(&self, id: &str) -> Result<Value> {
        let response = self.get_json(&format!("{API_URL}/novels/{id}"))?;
        response
            .get("novel")
            .cloned()
            .ok_or_else(|| Error::new("Novel Archive response has no novel"))
    }

    fn parse_chapters(id: &str, value: &Value) -> Result<Vec<NovelChapter>> {
        let names = value.get("chapter_names").and_then(Value::as_array);
        let count = names
            .map(|values| values.len() as u64)
            .or_else(|| number_value(value, "total_chapters"))
            .unwrap_or(0);
        require(
            (count > 0).then_some(()),
            "Novel Archive novel has no chapters",
        )?;
        Ok((1..=count)
            .map(|number| {
                let title = names
                    .and_then(|values| values.get((number - 1) as usize))
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_owned)
                    .or_else(|| Some(format!("Chapter {number}")));
                let url = format!("{BASE_URL}/reader?novel={id}&chapter={number}");
                NovelChapter {
                    key: format!("{id}:{number}"),
                    title,
                    chapter_number: Some(number as f32),
                    url: Some(url),
                    language: Some("en".into()),
                    source_order: Some((number - 1) as i32),
                    ..NovelChapter::default()
                }
            })
            .collect())
    }

    fn chapter_number(chapter: &NovelChapter) -> Result<u64> {
        if let Some(number) = chapter.chapter_number {
            return Ok(number as u64);
        }
        let candidate = chapter.url.as_deref().unwrap_or(&chapter.key);
        if let Ok(url) = Url::parse(candidate) {
            if let Some(number) = url
                .query_pairs()
                .find_map(|(key, value)| (key == "chapter").then(|| value.into_owned()))
                .and_then(|value| value.parse().ok())
            {
                return Ok(number);
            }
        }
        chapter
            .key
            .rsplit(':')
            .next()
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| Error::new("Novel Archive chapter has no number"))
    }
}

impl NovelSource for NovelArchiveSource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse(page, None, &json!({"sort": "popular"}))
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse(page, None, &json!({"sort": "recent"}))
    }

    fn listing(&mut self, listing: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        let mut filters = filters.clone();
        let sort = match listing {
            "popular" => filters
                .get("sort")
                .and_then(Value::as_str)
                .unwrap_or("popular"),
            "latest" => "recent",
            _ => return Err(Error::new(format!("unknown novel listing {listing:?}"))),
        };
        filters["sort"] = json!(sort);
        self.browse(page, None, &filters)
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        self.browse(page, Some(query), filters)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let id = Self::id(&item)?;
        let value = self.details_value(&id)?;
        require(
            (!is_restricted(&value)).then_some(()),
            "Novel Archive classified this title as adult content",
        )?;
        let mut item = Self::parse_item(&value)?;
        item.initialized = true;
        Ok(item)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        let id = Self::id(&item)?;
        let value = self.details_value(&id)?;
        require(
            (!is_restricted(&value)).then_some(()),
            "Novel Archive classified this title as adult content",
        )?;
        Self::parse_chapters(&id, &value)
    }

    fn chapters_page(&mut self, item: CatalogItem, page: u32) -> Result<NovelChapterPage> {
        let id = Self::id(&item)?;
        let value = self.details_value(&id)?;
        require(
            (!is_restricted(&value)).then_some(()),
            "Novel Archive classified this title as adult content",
        )?;
        let chapters = Self::parse_chapters(&id, &value)?;
        let page = page.max(1);
        let start = (page as usize - 1).saturating_mul(CHAPTER_PAGE_SIZE);
        let total = chapters.len();
        let entries = chapters
            .into_iter()
            .skip(start)
            .take(CHAPTER_PAGE_SIZE)
            .collect();
        let page_count = total.div_ceil(CHAPTER_PAGE_SIZE).max(1) as u32;
        Ok(NovelChapterPage {
            entries,
            has_next_page: start.saturating_add(CHAPTER_PAGE_SIZE) < total,
            page_count: Some(page_count),
        })
    }

    fn text(&mut self, item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let id = Self::id(&item)?;
        let number = Self::chapter_number(&chapter)?;
        let response = self.get_json(&format!("{API_URL}/novels/{id}/chapters/{number}"))?;
        let chapter_value = response
            .get("chapter")
            .ok_or_else(|| Error::new("Novel Archive response has no chapter"))?;
        let content = string_value(chapter_value, "content")
            .ok_or_else(|| Error::new("Novel Archive chapter has no readable content"))?;
        let rendered = paragraphs(&content);
        require(
            (!rendered.is_empty()).then_some(()),
            "Novel Archive chapter has no readable content",
        )?;
        Ok(NovelText {
            html: Some(rendered.clone()),
            title: string_value(chapter_value, "name").or(chapter.title),
            base_url: Some(format!("{BASE_URL}/reader?novel={id}&chapter={number}")),
            blocks: vec![NovelContentBlock::Text {
                text: rendered,
                html: true,
            }],
            ..NovelText::default()
        })
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![
            select_filter(
                "sort",
                "Sort By",
                &[
                    ("Popular", "popular"),
                    ("Recently Updated", "recent"),
                    ("Most Chapters", "chapters"),
                    ("Highest Rated", "rating"),
                ],
            ),
            select_filter(
                "status",
                "Status",
                &[
                    ("All", "all"),
                    ("Ongoing", "ongoing"),
                    ("Completed", "completed"),
                    ("Hiatus", "hiatus"),
                ],
            ),
            select_filter("genre", "Genre", GENRES),
        ])
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let url = Url::parse(candidate).map_err(|error| Error::new(error.to_string()))?;
        if url.host_str() != Some("novelarchive.cc") {
            return Ok(None);
        }
        let id = url
            .query_pairs()
            .find_map(|(key, value)| (key == "novel" || key == "id").then(|| value.into_owned()));
        let Some(id) = id else {
            return Ok(None);
        };
        let mut item = CatalogItem::new(id.clone(), "");
        item.url = Some(format!("{BASE_URL}/novel?id={id}"));
        item.language = Some("en".into());
        item.extra.insert("novelArchiveId".into(), json!(id));
        let chapter = url
            .query_pairs()
            .find_map(|(key, value)| (key == "chapter").then(|| value.into_owned()))
            .and_then(|value| value.parse::<u64>().ok())
            .map(|number| NovelChapter {
                key: format!("{}:{number}", item.key),
                chapter_number: Some(number as f32),
                url: Some(candidate.into()),
                language: Some("en".into()),
                ..NovelChapter::default()
            });
        Ok(Some(UrlResolveResult {
            item: Some(item),
            novel_chapter: chapter,
            ..UrlResolveResult::default()
        }))
    }
}

fn string_value(value: &Value, key: &str) -> Option<String> {
    match value.get(key)? {
        Value::String(value) if !value.trim().is_empty() => Some(value.trim().to_owned()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn number_value(value: &Value, key: &str) -> Option<u64> {
    value
        .get(key)
        .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
}

fn genres(value: &Value) -> Vec<String> {
    value
        .get("genres")
        .and_then(|value| match value {
            Value::Array(values) => Some(
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
            ),
            Value::String(value) => Some(
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

fn is_restricted(value: &Value) -> bool {
    genres(value).iter().any(|genre| {
        matches!(
            genre.trim().to_ascii_lowercase().as_str(),
            "adult" | "erotica" | "smut" | "explicit sex" | "ecchi"
        )
    })
}

fn normalize_status(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "ongoing" => "ongoing",
        "completed" | "complete" => "completed",
        "hiatus" | "on hiatus" => "hiatus",
        _ => "unknown",
    }
}

fn paragraphs(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .split("\n\n")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("<p>{}</p>", escape_html(&value.replace('\n', "<br>"))))
        .collect()
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
        .replace("&lt;br&gt;", "<br>")
}

fn select_filter(id: &str, name: &str, values: &[(&str, &str)]) -> FilterDefinition {
    FilterDefinition::Select {
        id: id.into(),
        name: name.into(),
        options: values
            .iter()
            .map(|(label, value)| OptionItem {
                label: (*label).into(),
                value: (*value).into(),
            })
            .collect(),
        default_index: 0,
    }
}

const GENRES: &[(&str, &str)] = &[
    ("All", ""),
    ("Action", "Action"),
    ("Adventure", "Adventure"),
    ("Comedy", "Comedy"),
    ("Drama", "Drama"),
    ("Fantasy", "Fantasy"),
    ("Historical", "Historical"),
    ("Horror", "Horror"),
    ("Martial Arts", "Martial Arts"),
    ("Mystery", "Mystery"),
    ("Psychological", "Psychological"),
    ("Romance", "Romance"),
    ("School Life", "School Life"),
    ("Sci-Fi", "Sci-Fi"),
    ("Slice of Life", "Slice of Life"),
    ("Supernatural", "Supernatural"),
    ("Tragedy", "Tragedy"),
    ("Xianxia", "Xianxia"),
    ("Wuxia", "Wuxia"),
];

#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel(SOURCE_ID, NovelArchiveSource::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_item_and_filters_restricted_genres() {
        let safe = json!({
            "id": "fixture",
            "title": "Fixture Novel",
            "author": "Fixture Author",
            "genres": "Action,Fantasy",
            "cover_url": "/api/novels/fixture/cover",
            "total_chapters": "2"
        });
        let item = NovelArchiveSource::parse_item(&safe).unwrap();
        assert_eq!(item.title, "Fixture Novel");
        assert_eq!(item.key, "fixture");
        assert!(!is_restricted(&safe));
        assert!(is_restricted(&json!({"genres": "Fantasy,Adult"})));
    }

    #[test]
    fn creates_chapters_and_sanitized_text() {
        let fixture = json!({
            "total_chapters": "2",
            "chapter_names": ["First", "Second"]
        });
        let chapters = NovelArchiveSource::parse_chapters("fixture", &fixture).unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[1].title.as_deref(), Some("Second"));
        let html = paragraphs("One & two\n\n<script>alert(1)</script>");
        assert!(html.contains("One &amp; two"));
        assert!(!html.contains("<script>"));
    }
}
