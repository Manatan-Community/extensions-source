use manatan_common::{absolute_url, attr, normalize_space, selector};
use manatan_sdk::{
    client::Client,
    html::{self, Html},
    model::{CatalogItem, NovelChapter, NovelText, Paged, UrlResolveResult},
    Error, NovelSource, Result,
};
use serde_json::{json, Value};
use std::collections::HashSet;
use url::Url;
use wordpress_novel::{chapter_text, first_text, image, number, rating};

const BASE: &str = "https://secondlifetranslations.com";
#[derive(Default)]
pub struct SecondLife {
    client: Client,
}
impl SecondLife {
    fn document(&self, url: &str) -> Result<Html> {
        let response = self.client.get(url).send()?.error_for_status()?;
        Ok(html::document(response.text()?))
    }
    fn catalog(doc: &Html, page_url: &str, page: u32) -> Result<Paged<CatalogItem>> {
        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        for a in doc.select(&selector(
            ".tl-container a[href*='/novel/'], .search-entry-title a[href*='/novel/']",
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
            .select(&selector("a.btn-page[href], a.next.page-numbers")?)
            .any(|a| {
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
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let page = page.max(1);
        let url = format!("{BASE}/translations/?pg={page}");
        Self::catalog(&self.document(&url)?, &url, page)
    }
    fn search(&mut self, query: &str, page: u32, _filters: &Value) -> Result<Paged<CatalogItem>> {
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
        Self::detail(&self.document(url)?, url)
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
