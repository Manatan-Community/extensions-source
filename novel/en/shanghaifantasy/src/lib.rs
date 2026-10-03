use manatan_common::{absolute_url, attr, normalize_space, selector};
use manatan_sdk::{
    client::{BrowserChallengePolicy, Client, Response},
    html::{self, Html},
    model::{CatalogItem, NovelChapter, NovelChapterPage, NovelText, Paged, UrlResolveResult},
    Error, NovelSource, Result,
};
use serde_json::{json, Value};
use url::Url;
use wordpress_novel::{chapter_text, first_text, image, number, rating};

const BASE: &str = "https://shanghaifantasy.com";
pub struct Shanghai {
    client: Client,
}
impl Default for Shanghai {
    fn default() -> Self {
        Self {
            client: Client::browser().cookies_for(BASE),
        }
    }
}
impl Shanghai {
    fn request(&self, url: &str) -> Result<Response> {
        self.client
            .get(url)
            .send_with_challenge(
                &BrowserChallengePolicy::cloudflare(BASE).profile("shanghai-fantasy"),
            )?
            .error_for_status()
    }
    fn document(&self, url: &str) -> Result<Html> {
        Ok(html::document(self.request(url)?.text()?))
    }
    fn catalog_url(query: &str, page: u32, latest: bool) -> Result<String> {
        let mut url = Url::parse(&format!("{BASE}/wp-json/fiction/v1/novels/"))
            .map_err(|e| Error::new(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("novelstatus", "")
            .append_pair("term", "")
            .append_pair("page", &page.max(1).to_string())
            .append_pair("orderby", if latest { "date" } else { "" })
            .append_pair("order", if latest { "desc" } else { "" })
            .append_pair("query", query.trim());
        Ok(url.to_string())
    }
    fn page_count(response: &Response) -> Result<u32> {
        response
            .header("X-WP-TotalPages")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Error::new("Shanghai Fantasy did not return pagination metadata"))
    }
    fn catalog(data: &Value, has_next: bool) -> Result<Paged<CatalogItem>> {
        let rows = data.as_array().ok_or_else(|| {
            Error::new("Shanghai Fantasy returned an unexpected catalog response")
        })?;
        let mut entries = Vec::new();
        for row in rows {
            let url = absolute_url(
                BASE,
                row.get("permalink")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::new("Novel permalink is missing"))?,
            )?;
            let title = plain(row.get("title").and_then(Value::as_str).unwrap_or_default());
            if title.is_empty() {
                return Err(Error::new("Novel title is missing"));
            }
            let mut item = CatalogItem::new(&url, title);
            item.url = Some(url.clone());
            item.language = Some("en".into());
            item.description = row.get("novelIntro").and_then(Value::as_str).map(plain);
            item.cover = row
                .get("novelImage")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(|s| absolute_url(BASE, s))
                .transpose()?
                .map(|s| image(s, &url));
            if let Some(tags) = row.get("novelGenres").and_then(Value::as_str) {
                item.tags = html::fragment(tags)
                    .select(&selector("span, a")?)
                    .map(html::text)
                    .map(|s| normalize_space(&s))
                    .collect();
            }
            item.content_rating = Some(rating(&item.tags).into());
            entries.push(item);
        }
        Ok(Paged::new(entries, has_next))
    }
    fn browse(&self, query: &str, page: u32, latest: bool) -> Result<Paged<CatalogItem>> {
        let response = self.request(&Self::catalog_url(query, page, latest)?)?;
        let pages = Self::page_count(&response)?;
        let data = serde_json::from_str(response.text()?)
            .map_err(|_| Error::new("Shanghai Fantasy catalog is not JSON"))?;
        Self::catalog(&data, page.max(1) < pages)
    }
    fn detail(doc: &Html, url: &str) -> Result<CatalogItem> {
        let title = first_text(doc, "p.font-secondary.font-bold")?
            .ok_or_else(|| Error::new("Shanghai Fantasy novel title is missing"))?;
        let mut item = CatalogItem::new(url, title);
        item.url = Some(url.into());
        item.language = Some("en".into());
        item.initialized = true;
        item.authors = doc
            .select(&selector("p")?)
            .map(html::text)
            .map(|s| normalize_space(&s))
            .filter_map(|s| {
                s.strip_prefix("Author:")
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            })
            .collect();
        item.description = first_text(doc, "[x-show*=Synopsis]")?;
        item.cover = doc
            .select(&selector("img[src*='/wp-content/uploads/']")?)
            .next()
            .and_then(|i| attr(i, "src"))
            .map(|s| absolute_url(BASE, &s))
            .transpose()?
            .map(|s| image(s, url));
        item.tags = doc
            .select(&selector("a[href*='genre=']")?)
            .map(html::text)
            .map(|s| normalize_space(&s))
            .filter(|s| !s.is_empty())
            .collect();
        item.content_rating = Some(rating(&item.tags).into());
        item.status = Some(json!(
            match first_text(doc, "a[href*='status=']")?.as_deref() {
                Some("Ongoing") => "ongoing",
                Some("Completed") => "completed",
                Some("Dropped") => "cancelled",
                _ => "unknown",
            }
        ));
        let category = doc
            .select(&selector("#chapterList[data-cat]")?)
            .next()
            .and_then(|e| attr(e, "data-cat"))
            .filter(|s| s.parse::<u64>().is_ok())
            .ok_or_else(|| Error::new("Shanghai Fantasy chapter category is missing"))?;
        item.extra.insert("chapterCategory".into(), json!(category));
        Ok(item)
    }
    fn chapter_rows(data: &Value) -> Result<Vec<NovelChapter>> {
        data.as_array()
            .ok_or_else(|| Error::new("Shanghai Fantasy returned an unexpected chapter response"))?
            .iter()
            .map(|row| {
                let url = absolute_url(
                    BASE,
                    row.get("permalink")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Error::new("Chapter permalink is missing"))?,
                )?;
                let title = plain(row.get("title").and_then(Value::as_str).unwrap_or_default());
                Ok(NovelChapter {
                    key: url.clone(),
                    url: Some(url),
                    chapter_number: number(&title),
                    title: Some(title),
                    is_locked: row.get("locked").and_then(Value::as_bool).unwrap_or(false),
                    language: Some("en".into()),
                    ..NovelChapter::default()
                })
            })
            .collect()
    }
    fn chapter_page(&self, item: CatalogItem, page: u32) -> Result<NovelChapterPage> {
        let item = if item.extra.contains_key("chapterCategory") {
            item
        } else {
            Self::detail(
                &self.document(item.url.as_deref().unwrap_or(&item.key))?,
                item.url.as_deref().unwrap_or(&item.key),
            )?
        };
        let category = item
            .extra
            .get("chapterCategory")
            .and_then(Value::as_str)
            .filter(|s| s.parse::<u64>().is_ok())
            .ok_or_else(|| Error::new("Chapter category is missing"))?;
        let response = self.request(&format!(
            "{BASE}/wp-json/fiction/v1/chapters?category={category}&order=desc&page={}&per_page=50",
            page.max(1)
        ))?;
        let pages = Self::page_count(&response)?;
        let data = serde_json::from_str(response.text()?)
            .map_err(|_| Error::new("Shanghai Fantasy chapter list is not JSON"))?;
        Ok(NovelChapterPage {
            entries: Self::chapter_rows(&data)?,
            has_next_page: page.max(1) < pages,
            page_count: Some(pages),
        })
    }
}
fn plain(s: &str) -> String {
    normalize_space(&html::text(html::fragment(s).root_element()))
}
impl NovelSource for Shanghai {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse("", page, false)
    }
    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse("", page, true)
    }
    fn search(&mut self, query: &str, page: u32, _filters: &Value) -> Result<Paged<CatalogItem>> {
        self.browse(query, page, false)
    }
    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let url = item.url.as_deref().unwrap_or(&item.key);
        Self::detail(&self.document(url)?, url)
    }
    fn chapters_page(&mut self, item: CatalogItem, page: u32) -> Result<NovelChapterPage> {
        self.chapter_page(item, page)
    }
    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        let mut entries = Vec::new();
        for page in 1..=2000 {
            let result = self.chapter_page(item.clone(), page)?;
            entries.extend(result.entries);
            if !result.has_next_page {
                return Ok(entries);
            }
        }
        Err(Error::new(
            "Shanghai Fantasy chapter pagination exceeded its safety limit",
        ))
    }
    fn text(&mut self, _item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        if chapter.is_locked {
            return Err(Error::new("This chapter is locked on Shanghai Fantasy. Open it on the website to sign in or unlock it."));
        }
        let url = chapter.url.as_deref().unwrap_or(&chapter.key);
        chapter_text(
            &self.document(url)?,
            "div.contenta",
            url,
            chapter.title.clone(),
        )
    }
    fn handle_url(&mut self, url: &str) -> Result<Option<UrlResolveResult>> {
        let Ok(parsed) = Url::parse(url) else {
            return Ok(None);
        };
        if parsed.host_str() != Some("shanghaifantasy.com") || !parsed.path().starts_with("/novel/")
        {
            return Ok(None);
        }
        let mut item = CatalogItem::new(url, "");
        item.url = Some(url.into());
        Ok(Some(UrlResolveResult {
            item: Some(item),
            ..UrlResolveResult::default()
        }))
    }
}
#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel("shanghaifantasy", Shanghai::default())
}
#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn details_extract_public_category_without_parsing_url_slugs() {
        let doc = html::document(
            r#"<p class="font-secondary font-bold">Test</p><p><span>Author: </span>Example Author</p><img src="/wp-content/uploads/cover.png"><a href="/library/?status=Ongoing">Ongoing</a><a href="/library/?genre=Fantasy">Fantasy</a><div x-show="activeTab==='Synopsis'"><p>Synopsis</p></div><ul id="chapterList" data-cat="1234"></ul>"#,
        );
        let item = Shanghai::detail(&doc, &format!("{BASE}/novel/test/")).unwrap();
        assert_eq!(item.authors, vec!["Example Author"]);
        assert_eq!(item.extra.get("chapterCategory"), Some(&json!("1234")));
        assert_eq!(item.status, Some(json!("ongoing")));
        assert_eq!(item.description.as_deref(), Some("Synopsis"));
    }
    #[test]
    fn parses_the_public_catalog_contract() {
        let data = json!([{ "permalink":"/novel/test/","title":"Test &amp; One","novelIntro":"<p>Synopsis</p>","novelImage":"/cover.jpg","novelGenres":"<span>Fantasy</span><span>Ecchi</span>"}]);
        let result = Shanghai::catalog(&data, true).unwrap();
        assert_eq!(result.entries[0].title, "Test & One");
        assert!(result.has_next_page);
        assert_eq!(result.entries[0].tags, vec!["Fantasy", "Ecchi"]);
        assert_eq!(result.entries[0].content_rating.as_deref(), Some("general"));
    }
    #[test]
    fn chapter_api_retains_locked_state_and_source_order() {
        let data = json!([{"permalink":"/test-chapter-2/","title":"Chapter 2","locked":true},{"permalink":"/test-chapter-1/","title":"Chapter 1","locked":false}]);
        let chapters = Shanghai::chapter_rows(&data).unwrap();
        assert!(chapters[0].is_locked);
        assert_eq!(chapters[1].chapter_number, Some(1.));
    }
    #[test]
    fn locked_chapters_never_request_paid_text() {
        let mut source = Shanghai::default();
        assert!(source
            .text(
                CatalogItem::default(),
                NovelChapter {
                    is_locked: true,
                    ..NovelChapter::default()
                }
            )
            .unwrap_err()
            .to_string()
            .contains("locked"));
    }
    #[test]
    fn searches_are_encoded_and_paging_is_newest_first() {
        let url = Shanghai::catalog_url("A & B", 2, false).unwrap();
        assert!(url.contains("query=A+%26+B"));
        assert!(url.contains("page=2"));
        assert!(Shanghai::catalog(&json!({"unexpected":true}), false).is_err());
    }
}
