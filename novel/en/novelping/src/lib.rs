use std::collections::BTreeSet;

use manatan_common::{attr, normalize_space, selector};
use manatan_sdk::{
    client::Client,
    html::{self, Html},
    model::{CatalogItem, NovelChapter, NovelText, Paged, UrlResolveResult},
    Error, NovelSource, Result,
};
use serde_json::{json, Value};
use url::Url;
use wordpress_novel::{chapter_text, first_text, image, number, rating};

const BASE: &str = "https://novelping.com";

pub struct NovelPing {
    client: Client,
}

fn book_link(candidate: &str) -> Option<(String, Option<String>)> {
    let mut url = Url::parse(BASE).ok()?.join(candidate).ok()?;
    if url.origin().ascii_serialization() != BASE
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    url.set_query(None);
    url.set_fragment(None);
    let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
    match parts.as_slice() {
        ["book", slug] if !slug.is_empty() => Some((format!("{BASE}/book/{slug}"), None)),
        ["book", slug, chapter] if !slug.is_empty() && !chapter.is_empty() => {
            let book = format!("{BASE}/book/{slug}");
            Some((book.clone(), Some(format!("{book}/{chapter}"))))
        }
        _ => None,
    }
}

fn meta(doc: &Html, property: &str) -> Result<Option<String>> {
    Ok(doc
        .select(&selector(&format!("meta[property='{property}']"))?)
        .find_map(|element| attr(element, "content")))
}

impl NovelPing {
    fn document(&self, url: &str) -> Result<Html> {
        let response = self.client.get(url).send()?.error_for_status()?;
        Ok(html::document(response.text()?))
    }

    fn catalog(&self, path: &str, query: &str, page: u32) -> Result<Paged<CatalogItem>> {
        let mut url = Url::parse(BASE).unwrap().join(path).unwrap();
        if !query.is_empty() {
            url.query_pairs_mut().append_pair("keyword", query);
        }
        if page > 1 {
            url.query_pairs_mut().append_pair("page", &page.to_string());
        }
        let doc = self.document(url.as_str())?;
        let root = doc
            .select(&selector(".list.list-novel.col-xs-12")?)
            .next()
            .ok_or_else(|| Error::new("NovelPing catalog is unavailable"))?;
        let title_link = selector(".novel-title a[href]")?;
        let cover = selector("img.cover[src]")?;
        let mut seen = BTreeSet::new();
        let entries = root
            .select(&selector(".row")?)
            .filter_map(|row| {
                let link = row.select(&title_link).next()?;
                let (key, _) = book_link(link.value().attr("href")?)?;
                let title = normalize_space(&html::text(link));
                if title.is_empty() || !seen.insert(key.clone()) {
                    return None;
                }
                let mut item = CatalogItem::new(&key, title);
                item.url = Some(key.clone());
                item.cover = row
                    .select(&cover)
                    .next()
                    .and_then(|img| attr(img, "src"))
                    .map(|src| image(src, &key));
                Some(item)
            })
            .collect();
        let next = doc
            .select(&selector(".pagination .next:not(.disabled) a[href]")?)
            .next()
            .is_some();
        Ok(Paged::new(entries, next))
    }
}

impl NovelSource for NovelPing {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.catalog("/sort/popular", "", page)
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.catalog("/sort/updates", "", page)
    }

    fn search(&mut self, query: &str, page: u32, _: &Value) -> Result<Paged<CatalogItem>> {
        if query.trim().is_empty() {
            return self.popular(page);
        }
        self.catalog("/search", query.trim(), page)
    }

    fn details(&mut self, mut item: CatalogItem) -> Result<CatalogItem> {
        let (book, _) =
            book_link(&item.key).ok_or_else(|| Error::new("Invalid NovelPing book URL"))?;
        let doc = self.document(&book)?;
        item.title = meta(&doc, "og:novel:novel_name")?
            .ok_or_else(|| Error::new("NovelPing book details are unavailable"))?;
        item.description = first_text(&doc, "#novel-description-content")?;
        item.authors = meta(&doc, "og:novel:author")?.into_iter().collect();
        if let Some(genres) = meta(&doc, "og:novel:genre")? {
            item.tags = genres
                .split(',')
                .map(str::trim)
                .filter(|genre| !genre.is_empty())
                .map(str::to_owned)
                .collect();
        }
        item.content_rating = Some(rating(&item.tags).into());
        item.status = match meta(&doc, "og:novel:status")?.as_deref() {
            Some("Completed") => Some(json!("completed")),
            Some("Ongoing") => Some(json!("ongoing")),
            _ => None,
        };
        item.cover = meta(&doc, "og:image")?
            .map(|src| image(src, &book))
            .or(item.cover);
        item.url = Some(book);
        item.initialized = true;
        Ok(item)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        let (book, _) =
            book_link(&item.key).ok_or_else(|| Error::new("Invalid NovelPing book URL"))?;
        let mut url = Url::parse(BASE)
            .unwrap()
            .join("/ajax/chapter-archive")
            .unwrap();
        url.query_pairs_mut()
            .append_pair("novelId", book.rsplit('/').next().unwrap());
        let doc = self.document(url.as_str())?;
        let title = selector(".chapter-title")?;
        let mut seen = BTreeSet::new();
        let mut chapters: Vec<_> = doc
            .select(&selector(".chapter-list-item a[href]")?)
            .filter_map(|link| {
                let (parent, Some(url)) = book_link(link.value().attr("href")?)? else {
                    return None;
                };
                if parent != book || !seen.insert(url.clone()) {
                    return None;
                }
                let title =
                    attr(link, "title").or_else(|| link.select(&title).next().map(html::text))?;
                Some(NovelChapter {
                    key: url.clone(),
                    chapter_number: number(&title),
                    title: Some(normalize_space(&title)),
                    url: Some(url),
                    ..Default::default()
                })
            })
            .collect();
        if chapters.is_empty() {
            return Err(Error::new("NovelPing chapter archive is unavailable"));
        }
        chapters.reverse();
        Ok(chapters)
    }

    fn text(&mut self, _: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let (_, url) =
            book_link(&chapter.key).ok_or_else(|| Error::new("Invalid NovelPing chapter URL"))?;
        let url = url.ok_or_else(|| Error::new("Invalid NovelPing chapter URL"))?;
        chapter_text(&self.document(&url)?, "#chr-content", &url, chapter.title)
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let Some((book, chapter)) = book_link(candidate) else {
            return Ok(None);
        };
        let mut item = CatalogItem::new(&book, "");
        item.url = Some(book);
        Ok(Some(UrlResolveResult {
            item: Some(self.details(item)?),
            chapter_key: chapter,
            ..Default::default()
        }))
    }
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(manatan_sdk::Extension::new().novel(
    "novelping",
    NovelPing {
        client: Client::browser()
    }
));
