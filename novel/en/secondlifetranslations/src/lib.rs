use manatan_common::{absolute_url, attr, normalize_space, selector};
use manatan_sdk::{
    client::Client,
    html::{self, Html},
    model::{
        CatalogItem, FilterDefinition, NovelChapter, NovelText, OptionItem, Paged, UrlResolveResult,
    },
    Error, NovelSource, Result,
};
use serde_json::{json, Value};
use std::collections::HashSet;
use url::Url;
use wordpress_novel::{chapter_text, first_text, image, number, rating};

const BASE: &str = "https://secondlifetranslations.com";
const AJAX: &str = "https://secondlifetranslations.com/wp-admin/admin-ajax.php";
const PAGE_SIZE: usize = 20;
#[derive(Default)]
pub struct SecondLife {
    client: Client,
}
impl SecondLife {
    fn original_language(filters: &Value) -> Result<&'static str> {
        match filters
            .get("originalLanguage")
            .and_then(Value::as_str)
            .unwrap_or("")
        {
            "" => Ok(""),
            "Chinese" => Ok("Chinese"),
            "Japanese" => Ok("Japanese"),
            _ => Err(Error::new(
                "Unsupported original language; choose Any, Chinese or Japanese",
            )),
        }
    }
    fn nonce(doc: &Html) -> Result<String> {
        for script in doc.select(&selector("script")?) {
            let text = html::text(script);
            let Some((_, config)) = text.split_once("var myAjax = ") else {
                continue;
            };
            let Some(config) = config.split(';').next() else {
                continue;
            };
            if let Ok(config) = serde_json::from_str::<Value>(config.trim()) {
                if let Some(nonce) = config
                    .get("nonce")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    return Ok(nonce.into());
                }
            }
        }
        Err(Error::new(
            "Second Life catalog filter configuration is missing",
        ))
    }
    fn filtered_page(&self, page: u32, language: &str, nonce: &str) -> Result<Paged<CatalogItem>> {
        let page_text = page.to_string();
        let response = self
            .client
            .post(AJAX)
            .form(&[
                ("action", "novelfilter"),
                ("pg", page_text.as_str()),
                ("language", language),
                ("status", ""),
                ("genre", ""),
                ("nonce", nonce),
            ])
            .send()?
            .error_for_status()?;
        let html = response.text()?;
        if html.trim() == "-1" || html.trim() == "0" {
            return Err(Error::new(
                "Second Life rejected the catalog filter; refresh and try again",
            ));
        }
        let mut catalog = Self::catalog(&html::document(html), BASE, page)?;
        for item in &mut catalog.entries {
            item.extra
                .insert("originalLanguage".into(), json!(language));
        }
        Ok(catalog)
    }
    fn filtered_search(
        &self,
        query: &str,
        page: u32,
        language: &str,
    ) -> Result<Paged<CatalogItem>> {
        let doc = self.document(&format!("{BASE}/translations/"))?;
        let nonce = Self::nonce(&doc)?;
        if query.trim().is_empty() {
            return self.filtered_page(page.max(1), language, &nonce);
        }
        // The website's filter endpoint has no text-search parameter. Search its
        // filtered catalog, not the unrelated WordPress search which drops language.
        let wanted = usize::try_from(page.max(1))
            .unwrap_or(usize::MAX)
            .checked_mul(PAGE_SIZE)
            .and_then(|n| n.checked_add(1))
            .ok_or_else(|| Error::new("Second Life search page is out of range"))?;
        let query = normalize_space(query).to_lowercase();
        let mut matches = Vec::new();
        let mut seen = HashSet::new();
        for catalog_page in 1..=50 {
            let result = self.filtered_page(catalog_page, language, &nonce)?;
            for item in result.entries {
                if item.title.to_lowercase().contains(&query) && seen.insert(item.key.clone()) {
                    matches.push(item);
                }
            }
            if matches.len() >= wanted || !result.has_next_page {
                let start = wanted - PAGE_SIZE - 1;
                let has_next = matches.len() > start + PAGE_SIZE;
                return Ok(Paged::new(
                    matches.into_iter().skip(start).take(PAGE_SIZE).collect(),
                    has_next,
                ));
            }
        }
        Err(Error::new("Second Life filtered search exceeded its catalog page limit; browse with the original-language filter instead"))
    }
    fn document(&self, url: &str) -> Result<Html> {
        let response = self.client.get(url).send()?.error_for_status()?;
        Ok(html::document(response.text()?))
    }
    fn catalog(doc: &Html, page_url: &str, page: u32) -> Result<Paged<CatalogItem>> {
        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        for a in doc.select(&selector(
            ".tl-container a[href*='/novel/'], .all-novels a[href*='/novel/'], .search-entry-title a[href*='/novel/']",
        )?) {
            let url = absolute_url(BASE, &attr(a, "href").unwrap_or_default())?;
            let title = normalize_space(&html::text(a));
            if title.is_empty() || !seen.insert(url.clone()) {
                continue;
            }
            let mut item = CatalogItem::new(&url, title);
            item.url = Some(url.clone());
            item.language = Some("en".into());
            item.cover = a
                .select(&selector("img")?)
                .next()
                .and_then(|i| attr(i, "data-src").or_else(|| attr(i, "src")))
                .map(|s| absolute_url(BASE, &s))
                .transpose()?
                .map(|s| image(s, &url));
            // Search results expose the same public genre classes as the detail page.
            item.tags = a
                .ancestors()
                .filter_map(html::ElementRef::wrap)
                .find(|el| el.value().name() == "article")
                .map(|el| {
                    el.value()
                        .classes()
                        .filter_map(|c| c.strip_prefix("genres-"))
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            item.content_rating = Some(rating(&item.tags).into());
            entries.push(item);
        }
        let has_next = doc
            .select(&selector(
                "a.btn-page[href], a.btn-page[p], a.next.page-numbers",
            )?)
            .any(|a| {
                if attr(a, "p")
                    .and_then(|s| s.parse::<u32>().ok())
                    .is_some_and(|n| n > page)
                {
                    return true;
                }
                attr(a, "href")
                    .and_then(|s| Url::parse(page_url).ok()?.join(&s).ok())
                    .is_some_and(|u| {
                        u.query_pairs()
                            .find(|(k, _)| k == "pg" || k == "paged")
                            .and_then(|(_, v)| v.parse::<u32>().ok())
                            .is_some_and(|n| n > page)
                            || u.path().contains(&format!("/page/{}/", page + 1))
                    })
            });
        Ok(Paged::new(entries, has_next))
    }
    fn detail(doc: &Html, url: &str) -> Result<CatalogItem> {
        let mut item = CatalogItem::new(
            url,
            first_text(doc, ".noveltitle h1")?
                .ok_or_else(|| Error::new("Second Life novel title is missing"))?,
        );
        item.url = Some(url.into());
        item.language = Some("en".into());
        item.initialized = true;
        item.cover = doc
            .select(&selector("img.novelcover")?)
            .next()
            .and_then(|i| attr(i, "src"))
            .map(|s| absolute_url(BASE, &s))
            .transpose()?
            .map(|s| image(s, url));
        item.description = first_text(doc, ".novel-entry-content")?;
        item.tags = doc
            .select(&selector(".novelgenres a")?)
            .map(html::text)
            .map(|s| normalize_space(&s))
            .collect();
        item.content_rating = Some(rating(&item.tags).into());
        item.status = Some(json!(match first_text(doc, ".cstatus")?.as_deref() {
            Some("Completed") => "completed",
            Some("Ongoing") => "ongoing",
            Some("Hiatus") => "hiatus",
            Some("Dropped") => "cancelled",
            _ => "unknown",
        }));
        if let Some(info) = doc.select(&selector(".novelcontent")?).next() {
            // Author is a text node after its label, not an external raw-source link.
            if let Some(label) = info
                .select(&selector("strong")?)
                .find(|e| normalize_space(&html::text(*e)) == "Author:")
            {
                let mut value = String::new();
                for node in label.next_siblings() {
                    if html::ElementRef::wrap(node).is_some_and(|e| e.value().name() == "br") {
                        break;
                    }
                    if let Some(t) = node.value().as_text() {
                        value.push_str(t);
                    }
                }
                let value = normalize_space(&value);
                if !value.is_empty() {
                    item.authors.push(value);
                }
            }
        }
        Ok(item)
    }
    fn chapter_list(doc: &Html) -> Result<Vec<NovelChapter>> {
        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        for a in doc.select(&selector(".panel a[href]")?) {
            let title = normalize_space(&html::text(a));
            if title.is_empty() {
                continue;
            }
            let url = absolute_url(BASE, &attr(a, "href").unwrap_or_default())?;
            if Url::parse(&url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .as_deref()
                != Some("secondlifetranslations.com")
                || !seen.insert(url.clone())
            {
                continue;
            }
            entries.push(NovelChapter {
                key: url.clone(),
                url: Some(url),
                chapter_number: number(&title),
                title: Some(title),
                language: Some("en".into()),
                ..NovelChapter::default()
            });
        }
        if entries.is_empty() {
            return Err(Error::new("Second Life returned no chapter links; open the novel on the website to check availability"));
        }
        entries.reverse();
        Ok(entries)
    }
}
impl NovelSource for SecondLife {
    fn listing(&mut self, listing: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        if listing != "popular" {
            return Err(Error::new("Unknown Second Life listing"));
        }
        self.search("", page, filters)
    }
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let page = page.max(1);
        let url = format!("{BASE}/translations/?pg={page}");
        Self::catalog(&self.document(&url)?, &url, page)
    }
    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        let language = Self::original_language(filters)?;
        if !language.is_empty() {
            return self.filtered_search(query, page, language);
        }
        if query.trim().is_empty() {
            return self.popular(page);
        }
        let mut url = Url::parse(BASE).map_err(|e| Error::new(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("s", query.trim())
            .append_pair("post_type", "novel")
            .append_pair("paged", &page.max(1).to_string());
        Self::catalog(&self.document(url.as_str())?, url.as_str(), page.max(1))
    }
    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let url = item.url.as_deref().unwrap_or(&item.key);
        let mut details = Self::detail(&self.document(url)?, url)?;
        details.extra = item.extra;
        Ok(details)
    }
    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        Self::chapter_list(&self.document(item.url.as_deref().unwrap_or(&item.key))?)
    }
    fn text(&mut self, _item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let url = chapter.url.as_deref().unwrap_or(&chapter.key);
        chapter_text(
            &self.document(url)?,
            ".entry-content[itemprop='text']",
            url,
            chapter.title.clone(),
        )
    }
    fn handle_url(&mut self, url: &str) -> Result<Option<UrlResolveResult>> {
        let Ok(parsed) = Url::parse(url) else {
            return Ok(None);
        };
        if parsed.host_str() != Some("secondlifetranslations.com")
            || !parsed.path().starts_with("/novel/")
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
    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![FilterDefinition::Select {
            id: "originalLanguage".into(),
            name: "Original language (translated to English)".into(),
            options: [
                ("Any", ""),
                ("Chinese", "Chinese"),
                ("Japanese", "Japanese"),
            ]
            .into_iter()
            .map(|(label, value)| OptionItem {
                label: label.into(),
                value: value.into(),
            })
            .collect(),
            default_index: 0,
        }])
    }
}
#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel("secondlifetranslations", SecondLife::default())
}
#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_language_is_distinct_from_reading_language() {
        let mut source = SecondLife::default();
        assert!(source
            .listing("popular", 1, &json!({"originalLanguage":"Korean"}))
            .is_err());
        assert!(source.listing("unknown", 1, &json!({})).is_err());
        assert_eq!(
            SecondLife::original_language(&json!({"originalLanguage":"Japanese"})).unwrap(),
            "Japanese"
        );
        assert_eq!(
            SecondLife::original_language(&json!({"originalLanguage":"Chinese"})).unwrap(),
            "Chinese"
        );
        assert!(SecondLife::original_language(&json!({"originalLanguage":"Korean"})).is_err());
        let doc = html::document(
            r#"<div class="all-novels"><a href="/novel/test/"><h6>Japanese original</h6></a></div><a class="btn btn-page" p="2">Next</a>"#,
        );
        let catalog = SecondLife::catalog(&doc, BASE, 1).unwrap();
        assert_eq!(catalog.entries.len(), 1);
        assert_eq!(catalog.entries[0].language.as_deref(), Some("en"));
        assert!(catalog.has_next_page);
        assert!(!SecondLife::catalog(&doc, BASE, 2).unwrap().has_next_page);
    }
    #[test]
    fn filter_nonce_is_parsed_as_data_and_never_executed() {
        let doc = html::document(
            r#"<script>var myAjax = {"nonce":"fixture-nonce","ajaxurl":"https://untrusted.invalid"};</script>"#,
        );
        assert_eq!(SecondLife::nonce(&doc).unwrap(), "fixture-nonce");
        assert!(SecondLife::nonce(&html::document("<script>other()</script>")).is_err());
    }
    #[test]
    fn details_keep_author_status_and_explicit_content_rating() {
        let doc = html::document(
            r#"<div class="noveltitle"><h1>Test</h1></div><img class="novelcover" src="/cover.png"><div class="novelcontent"><strong>Author: </strong>Example Author<br><span class="cstatus">Completed</span><span class="novelgenres"><a>Smut</a></span></div><div class="novel-entry-content"><p>Synopsis</p></div>"#,
        );
        let item = SecondLife::detail(&doc, &format!("{BASE}/novel/test/")).unwrap();
        assert_eq!(item.authors, vec!["Example Author"]);
        assert_eq!(item.status, Some(json!("completed")));
        assert_eq!(item.content_rating.as_deref(), Some("adult"));
    }
    #[test]
    fn catalog_paging_deduplicates_and_search_does_not_include_chapter_posts() {
        let doc = html::document(
            r#"<div class="tl-container"><a href="/novel/test/"><img src="/cover.jpg"><h6>Test Novel</h6></a><a href="/novel/test/">duplicate</a></div><a class="btn-page" href="?pg=2">Next</a>"#,
        );
        let result = SecondLife::catalog(&doc, BASE, 1).unwrap();
        assert_eq!(result.entries.len(), 1);
        assert!(result.has_next_page);
        assert!(result.entries[0].cover.is_some());
        assert!(!SecondLife::catalog(&doc, BASE, 2).unwrap().has_next_page);
    }
    #[test]
    fn chapter_order_preserves_split_parts_and_extras() {
        let doc = html::document(
            r#"<div class="panel"><a href="/a/chapter-1/">Chapter 1</a><a href="/a/chapter-2-1/">Chapter 2.1</a><a href="/a/chapter-2-2/">Chapter 2.2</a><a href="/a/extra/">Extra 1</a><a href="/a/extra/">duplicate</a></div>"#,
        );
        let chapters = SecondLife::chapter_list(&doc).unwrap();
        assert_eq!(chapters.len(), 4);
        assert_eq!(chapters[0].title.as_deref(), Some("Extra 1"));
        assert_eq!(chapters[1].chapter_number, Some(2.2));
        assert_eq!(chapters[3].chapter_number, Some(1.));
    }
}
