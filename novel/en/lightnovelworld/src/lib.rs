use manatan_common::{absolute_url, attr, normalize_space, require, selector};
use manatan_sdk::{
    client::Client,
    html::{self, Html},
    model::{
        CatalogItem, ImageRequest, ImageRequestContext, NovelChapter, NovelChapterPage,
        NovelContentBlock, NovelText, Paged, UrlResolveResult,
    },
    Error, NovelSource, Result,
};
use serde_json::{json, Value};
use url::Url;

#[cfg(target_arch = "wasm32")]
const SOURCE_ID: &str = "lightnovelworld";
const BASE_URL: &str = "https://lightnovelworld.org";
const SEARCH_PAGE_SIZE: usize = 36;
const CHAPTER_PAGE_SIZE: u64 = 200;

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
    fn document(&self, url: &str) -> Result<(Html, String)> {
        let response = self.client.get(url).send()?.error_for_status()?;
        let final_url = response.final_url().to_owned();
        Ok((html::document(response.text()?), final_url))
    }

    fn listing_page(&self, listing: &str, page: u32) -> Result<Paged<CatalogItem>> {
        let page = page.max(1);
        let url = match listing {
            "popular" => format!("{BASE_URL}/ranking/?sort=rank&page={page}"),
            "latest" => format!("{BASE_URL}/updates/?page={page}"),
            _ => return Err(Error::new(format!("unknown novel listing {listing:?}"))),
        };
        let (document, _) = self.document(&url)?;
        let items = match listing {
            "popular" => Self::parse_ranking(&document)?,
            "latest" => Self::parse_updates(&document)?,
            _ => unreachable!(),
        };
        let has_next = has_next_page(&document, page)?;
        Ok(Paged::new(items, has_next))
    }

    fn parse_ranking(document: &Html) -> Result<Vec<CatalogItem>> {
        let cards = selector(".ranking-card")?;
        let links = selector("a.card-link[href]")?;
        let titles = selector(".card-title")?;
        let covers = selector(".card-cover[data-bg-image]")?;
        let genres = selector(".genre-tag")?;
        let statuses = selector(".status-badge")?;
        let mut items = Vec::new();
        for card in document.select(&cards) {
            let Some(href) = card.select(&links).find_map(|node| attr(node, "href")) else {
                continue;
            };
            let Some(title) = card
                .select(&titles)
                .next()
                .map(html::text)
                .map(|value| normalize_space(&value))
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let tags = card
                .select(&genres)
                .map(html::text)
                .map(|value| normalize_space(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            if restricted_tags(&tags) {
                continue;
            }
            let page_url = absolute_url(BASE_URL, &href)?;
            let mut item = CatalogItem::new(page_url.clone(), title);
            item.url = Some(page_url.clone());
            item.tags = tags;
            item.status = card
                .select(&statuses)
                .next()
                .map(html::text)
                .map(|value| json!(normalize_status(&value)));
            item.cover = card
                .select(&covers)
                .find_map(|node| attr(node, "data-bg-image"))
                .map(|cover| absolute_url(BASE_URL, &cover))
                .transpose()?
                .map(|cover| image(&cover, &page_url));
            item.language = Some("en".into());
            item.content_rating = Some("suggestive".into());
            items.push(item);
        }
        Ok(items)
    }

    fn parse_updates(document: &Html) -> Result<Vec<CatalogItem>> {
        let cards = selector("a.ranking-item.chapter-item[href]")?;
        let titles = selector(".ranking-item-title")?;
        let covers = selector("img[src]")?;
        let mut items = Vec::new();
        for card in document.select(&cards) {
            let Some(href) = attr(card, "href") else {
                continue;
            };
            let Some(title) = card
                .select(&titles)
                .next()
                .map(html::text)
                .map(|value| normalize_space(&value))
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let page_url = absolute_url(BASE_URL, &href)?;
            let mut item = CatalogItem::new(page_url.clone(), title);
            item.url = Some(page_url.clone());
            item.cover = card
                .select(&covers)
                .find_map(|node| attr(node, "src"))
                .map(|cover| absolute_url(BASE_URL, &cover))
                .transpose()?
                .map(|cover| image(&cover, &page_url));
            item.language = Some("en".into());
            item.content_rating = Some("suggestive".into());
            if !items
                .iter()
                .any(|existing: &CatalogItem| existing.key == item.key)
            {
                items.push(item);
            }
        }
        Ok(items)
    }

    fn parse_search(value: &Value) -> Result<Vec<CatalogItem>> {
        value
            .get("novels")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::new("Light Novel World search response has no novels"))?
            .iter()
            .filter(|value| {
                let tags = value
                    .get("genres")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                !restricted_tags(&tags)
            })
            .map(|value| {
                let slug = value
                    .get("slug")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::new("Light Novel World search result has no slug"))?;
                let title = value
                    .get("title")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::new("Light Novel World search result has no title"))?;
                let page_url = format!("{BASE_URL}/novel/{slug}/");
                let mut item = CatalogItem::new(page_url.clone(), title);
                item.url = Some(page_url.clone());
                item.authors = value
                    .get("author")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .into_iter()
                    .collect();
                item.tags = value
                    .get("genres")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                item.cover = value
                    .get("cover_path")
                    .and_then(Value::as_str)
                    .map(|cover| absolute_url(BASE_URL, cover))
                    .transpose()?
                    .map(|cover| image(&cover, &page_url));
                item.status = value
                    .get("status")
                    .and_then(Value::as_str)
                    .map(|value| json!(normalize_status(value)));
                item.language = Some("en".into());
                item.content_rating = Some("suggestive".into());
                Ok(item)
            })
            .collect()
    }

    fn item_url(item: &CatalogItem) -> Result<String> {
        let candidate = item.url.as_deref().unwrap_or(&item.key);
        let mut url = Url::parse(&absolute_url(BASE_URL, candidate)?)
            .map_err(|error| Error::new(error.to_string()))?;
        url.set_query(None);
        url.set_fragment(None);
        let path = url.path().to_owned();
        if let Some(index) = path.find("/chapter/") {
            url.set_path(&path[..index + 1]);
        } else if path.ends_with("/chapters/") {
            url.set_path(path.trim_end_matches("chapters/"));
        }
        Ok(url.to_string())
    }

    fn parse_details(document: &Html, page_url: &str) -> Result<CatalogItem> {
        let title = first_text(document, ".novel-title")?
            .ok_or_else(|| Error::new("Light Novel World novel has no title"))?;
        let author = first_text(document, ".novel-author")?
            .map(|value| value.trim_start_matches("Author:").trim().to_owned());
        let tags = texts(document, ".novel-genres .genre-tag")?;
        require(
            (!restricted_tags(&tags)).then_some(()),
            "Light Novel World classified this title as adult content",
        )?;
        let description = first_text(document, ".summary-content")?
            .or(first_text(document, ".description-text")?);
        let cover = first_attr(document, ".novel-cover-container img", "src")?
            .map(|cover| absolute_url(BASE_URL, &cover))
            .transpose()?;
        let mut item = CatalogItem::new(page_url, title);
        item.url = Some(page_url.into());
        item.authors = author
            .filter(|value| !value.is_empty())
            .into_iter()
            .collect();
        item.tags = tags;
        item.description = description;
        item.cover = cover.map(|cover| image(&cover, page_url));
        item.status = first_text(
            document,
            ".novel-meta .status-badge, .card-status .status-badge",
        )?
        .map(|value| json!(normalize_status(&value)));
        item.initialized = true;
        item.language = Some("en".into());
        item.content_rating = Some("suggestive".into());
        Ok(item)
    }

    fn slug(item: &CatalogItem) -> Result<String> {
        let url =
            Url::parse(&Self::item_url(item)?).map_err(|error| Error::new(error.to_string()))?;
        let segments = url
            .path_segments()
            .map(|segments| segments.collect::<Vec<_>>())
            .unwrap_or_default();
        segments
            .windows(2)
            .find_map(|pair| (pair[0] == "novel").then(|| pair[1].to_owned()))
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::new("Light Novel World URL has no novel slug"))
    }

    fn chapter_page(&self, slug: &str, offset: u64) -> Result<Value> {
        self.client
            .get(format!(
                "{BASE_URL}/api/novel/{slug}/chapters/?offset={offset}&limit={CHAPTER_PAGE_SIZE}"
            ))
            .send()?
            .error_for_status()?
            .json()
    }

    fn parse_text(document: &Html, chapter_url: &str) -> Result<NovelText> {
        let content = first_inner_html(document, ".chapter-content")?
            .ok_or_else(|| Error::new(
                "Light Novel World requires a free account to read chapters. Open Web View, sign in, then retry."
            ))?;
        let rendered = sanitize_html(&content)?;
        require(
            (!normalize_space(&html::text(html::fragment(&rendered).root_element())).is_empty())
                .then_some(()),
            "Light Novel World chapter has no readable content",
        )?;
        Ok(NovelText {
            html: Some(rendered.clone()),
            title: first_text(document, ".chapter-title")?,
            base_url: Some(chapter_url.into()),
            image_context: Some(ImageRequestContext {
                headers: [("Referer".into(), chapter_url.into())]
                    .into_iter()
                    .collect(),
                cookie_url: Some(BASE_URL.into()),
            }),
            blocks: vec![NovelContentBlock::Text {
                text: rendered,
                html: true,
            }],
            ..NovelText::default()
        })
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

    fn search(&mut self, query: &str, page: u32, _filters: &Value) -> Result<Paged<CatalogItem>> {
        let mut url = Url::parse(&format!("{BASE_URL}/api/search/"))
            .map_err(|error| Error::new(error.to_string()))?;
        url.query_pairs_mut()
            .append_pair("q", query.trim())
            .append_pair("search_type", "title");
        let response: Value = self
            .client
            .get(url.as_str())
            .send()?
            .error_for_status()?
            .json()?;
        let items = Self::parse_search(&response)?;
        let page = page.max(1) as usize;
        let start = (page - 1) * SEARCH_PAGE_SIZE;
        let total = items.len();
        Ok(Paged::new(
            items
                .into_iter()
                .skip(start)
                .take(SEARCH_PAGE_SIZE)
                .collect(),
            start + SEARCH_PAGE_SIZE < total,
        ))
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let url = Self::item_url(&item)?;
        let (document, final_url) = self.document(&url)?;
        Self::parse_details(&document, &final_url)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        let slug = Self::slug(&item)?;
        let mut offset = 0u64;
        let mut chapters = Vec::new();
        loop {
            let response = self.chapter_page(&slug, offset)?;
            let values = response
                .get("chapters")
                .and_then(Value::as_array)
                .ok_or_else(|| Error::new("Light Novel World response has no chapters"))?;
            for value in values {
                let Some(number) = value.get("number").and_then(Value::as_u64) else {
                    continue;
                };
                let title = value
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| Some(format!("Chapter {number}")));
                let url = format!("{BASE_URL}/novel/{slug}/chapter/{number}/");
                chapters.push(NovelChapter {
                    key: url.clone(),
                    title,
                    chapter_number: Some(number as f32),
                    url: Some(url),
                    language: Some("en".into()),
                    source_order: Some((number - 1) as i32),
                    ..NovelChapter::default()
                });
            }
            if !response
                .get("has_more")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                break;
            }
            offset += values.len() as u64;
            if values.is_empty() {
                break;
            }
        }
        require(
            (!chapters.is_empty()).then_some(()),
            "Light Novel World novel has no chapters",
        )?;
        Ok(chapters)
    }

    fn chapters_page(&mut self, item: CatalogItem, page: u32) -> Result<NovelChapterPage> {
        let slug = Self::slug(&item)?;
        let page = page.max(1);
        let offset = u64::from(page - 1) * CHAPTER_PAGE_SIZE;
        let response = self.chapter_page(&slug, offset)?;
        let values = response
            .get("chapters")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::new("Light Novel World response has no chapters"))?;
        let entries = values
            .iter()
            .filter_map(|value| {
                let number = value.get("number").and_then(Value::as_u64)?;
                let title = value
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| Some(format!("Chapter {number}")));
                let url = format!("{BASE_URL}/novel/{slug}/chapter/{number}/");
                Some(NovelChapter {
                    key: url.clone(),
                    title,
                    chapter_number: Some(number as f32),
                    url: Some(url),
                    language: Some("en".into()),
                    source_order: Some(number.saturating_sub(1) as i32),
                    ..NovelChapter::default()
                })
            })
            .collect::<Vec<_>>();
        let total = response
            .get("total_chapters")
            .and_then(Value::as_u64)
            .unwrap_or(offset + entries.len() as u64);
        let page_count = total
            .div_ceil(CHAPTER_PAGE_SIZE)
            .max(u64::from(page))
            .min(u64::from(u32::MAX)) as u32;
        Ok(NovelChapterPage {
            entries,
            has_next_page: response
                .get("has_more")
                .and_then(Value::as_bool)
                .unwrap_or(offset + CHAPTER_PAGE_SIZE < total),
            page_count: Some(page_count),
        })
    }

    fn text(&mut self, _item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let url = chapter.url.as_deref().unwrap_or(&chapter.key);
        let (document, final_url) = self.document(url)?;
        if !final_url.contains("/chapter/") {
            return Err(Error::new(
                "Light Novel World requires a free account to read chapters. Open Web View, sign in, then retry.",
            ));
        }
        Self::parse_text(&document, &final_url)
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let url = Url::parse(candidate).map_err(|error| Error::new(error.to_string()))?;
        if url.host_str() != Some("lightnovelworld.org") {
            return Ok(None);
        }
        let segments = url
            .path_segments()
            .map(|segments| segments.collect::<Vec<_>>())
            .unwrap_or_default();
        let Some(index) = segments.iter().position(|value| *value == "novel") else {
            return Ok(None);
        };
        let Some(slug) = segments.get(index + 1).filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        let item_url = format!("{BASE_URL}/novel/{slug}/");
        let mut item = CatalogItem::new(item_url.clone(), "");
        item.url = Some(item_url);
        item.language = Some("en".into());
        let chapter = segments
            .iter()
            .position(|value| *value == "chapter")
            .and_then(|index| segments.get(index + 1))
            .and_then(|value| value.parse::<u64>().ok())
            .map(|number| NovelChapter {
                key: candidate.into(),
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

fn image(url: &str, referer: &str) -> ImageRequest {
    ImageRequest::get(url)
        .header("Referer", referer)
        .cookies_for(BASE_URL)
}

fn first_text(document: &Html, query: &str) -> Result<Option<String>> {
    let query = selector(query)?;
    Ok(document
        .select(&query)
        .next()
        .map(html::text)
        .map(|value| normalize_space(&value))
        .filter(|value| !value.is_empty()))
}

fn texts(document: &Html, query: &str) -> Result<Vec<String>> {
    let query = selector(query)?;
    Ok(document
        .select(&query)
        .map(html::text)
        .map(|value| normalize_space(&value))
        .filter(|value| !value.is_empty())
        .collect())
}

fn first_attr(document: &Html, query: &str, name: &str) -> Result<Option<String>> {
    let query = selector(query)?;
    Ok(document
        .select(&query)
        .find_map(|element| attr(element, name)))
}

fn first_inner_html(document: &Html, query: &str) -> Result<Option<String>> {
    let query = selector(query)?;
    Ok(document.select(&query).next().map(|node| node.inner_html()))
}

fn has_next_page(document: &Html, page: u32) -> Result<bool> {
    let links = selector(".pagination a[href]")?;
    for link in document.select(&links) {
        let Some(href) = attr(link, "href") else {
            continue;
        };
        if let Ok(url) = Url::parse(&absolute_url(BASE_URL, &href)?) {
            if url
                .query_pairs()
                .find_map(|(key, value)| (key == "page").then(|| value.into_owned()))
                .and_then(|value| value.parse::<u32>().ok())
                == Some(page + 1)
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn restricted_tags(tags: &[String]) -> bool {
    tags.iter().any(|tag| {
        matches!(
            tag.trim().to_ascii_lowercase().as_str(),
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

fn sanitize_html(value: &str) -> Result<String> {
    let mut rendered = value.to_owned();
    for tag in ["script", "iframe", "object", "embed", "style", "form"] {
        let pattern = regex::Regex::new(&format!(
            r"(?is)<{tag}\b[^>]*>.*?</{tag}\s*>|<{tag}\b[^>]*/?>"
        ))
        .map_err(|error| Error::new(error.to_string()))?;
        rendered = pattern.replace_all(&rendered, "").into_owned();
    }
    let event = regex::Regex::new(r#"(?i)\s+on[a-z]+\s*=\s*(?:\"[^\"]*\"|'[^']*')"#)
        .map_err(|error| Error::new(error.to_string()))?;
    Ok(event.replace_all(&rendered, "").into_owned())
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

    #[test]
    fn parses_ranking_and_details_fixtures() {
        let ranking = html::document(include_str!("../tests/fixtures/ranking.html"));
        let items = LightNovelWorldSource::parse_ranking(&ranking).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Fixture Novel");
        let details = html::document(include_str!("../tests/fixtures/details.html"));
        let item = LightNovelWorldSource::parse_details(
            &details,
            "https://lightnovelworld.org/novel/fixture/",
        )
        .unwrap();
        assert_eq!(item.authors, vec!["Fixture Author"]);
        assert!(item.initialized);
    }

    #[test]
    fn parses_search_and_sanitizes_text_fixture() {
        let items = LightNovelWorldSource::parse_search(&json!({
            "novels": [{
                "slug": "fixture",
                "title": "Fixture Novel",
                "author": "Fixture Author",
                "genres": ["Fantasy"],
                "cover_path": "/fixture.jpg",
                "status": "Ongoing"
            }]
        }))
        .unwrap();
        assert_eq!(items[0].title, "Fixture Novel");
        let chapter = html::document(include_str!("../tests/fixtures/chapter.html"));
        let text = LightNovelWorldSource::parse_text(
            &chapter,
            "https://lightnovelworld.org/novel/fixture/chapter/1/",
        )
        .unwrap();
        let rendered = text.html.unwrap();
        assert!(rendered.contains("Readable fixture"));
        assert!(!rendered.contains("<script"));
        assert!(!rendered.contains("onclick"));
    }
}
