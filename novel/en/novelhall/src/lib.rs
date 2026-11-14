use std::collections::BTreeSet;

use manatan_common::{attr, normalize_space, selector};
use manatan_sdk::{
    client::{BrowserChallengePolicy, Client},
    html::{self, Html},
    model::{CatalogItem, NovelChapter, NovelText, Paged, UrlResolveResult},
    Error, NovelSource, Result,
};
use serde_json::{json, Value};
use url::Url;
use wordpress_novel::{chapter_text, first_text, image, number, rating};

const BASE: &str = "https://www.novelhall.com";

pub struct NovelHall {
    client: Client,
}

fn book_link(candidate: &str) -> Option<(String, Option<String>)> {
    let url = Url::parse(BASE).ok()?.join(candidate).ok()?;
    if !matches!(
        url.origin().ascii_serialization().as_str(),
        BASE | "https://novelhall.com"
    ) || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let mut parts = url.path().trim_matches('/').split('/');
    let slug = parts.next()?;
    let (_, id) = slug.rsplit_once('-')?;
    if !id.bytes().all(|byte| byte.is_ascii_digit()) || id.is_empty() {
        return None;
    }
    let book = format!("{BASE}/{}/", slug.to_ascii_lowercase());
    match (parts.next(), parts.next()) {
        (None, None) => Some((book, None)),
        (Some(chapter), None)
            if chapter.strip_suffix(".html").is_some_and(|id| {
                !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())
            }) =>
        {
            Some((book.clone(), Some(format!("{book}{chapter}"))))
        }
        _ => None,
    }
}

impl NovelHall {
    fn document(&self, url: &str) -> Result<Html> {
        let response = self
            .client
            .get(url)
            .send_with_challenge(&BrowserChallengePolicy::cloudflare(BASE).profile("novelhall"))?
            .error_for_status()?;
        Ok(html::document(response.text()?))
    }

    fn catalog(&self, url: &str, css: &str) -> Result<Paged<CatalogItem>> {
        let doc = self.document(url)?;
        let mut seen = BTreeSet::new();
        let entries = doc
            .select(&selector(css)?)
            .filter_map(|link| {
                let (book, _) = book_link(link.value().attr("href")?)?;
                let title = normalize_space(&html::text(link));
                if title.is_empty() || !seen.insert(book.clone()) {
                    return None;
                }
                let mut item = CatalogItem::new(&book, title);
                item.url = Some(book);
                // The listing has no artwork URL. Manatan can load it with details.
                Some(item)
            })
            .collect();
        let has_next = doc
            .select(&selector(".page-nav a[rel=next]")?)
            .next()
            .is_some();
        Ok(Paged::new(entries, has_next))
    }
}

impl NovelSource for NovelHall {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.catalog(
            &format!("{BASE}/all2022-{}.html", page.max(1)),
            "li.btm a[href]",
        )
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        if page > 1 {
            return Ok(Paged::new(Vec::new(), false));
        }
        self.catalog(
            &format!("{BASE}/lastupdate.html"),
            ".section3 td.w70 > a[href]",
        )
    }

    fn search(&mut self, query: &str, page: u32, _: &Value) -> Result<Paged<CatalogItem>> {
        if query.trim().is_empty() {
            return self.popular(page);
        }
        if page > 1 {
            return Ok(Paged::new(Vec::new(), false));
        }
        let mut url = Url::parse(BASE).unwrap().join("/index.php").unwrap();
        url.query_pairs_mut()
            .append_pair("s", "so")
            .append_pair("module", "book")
            .append_pair("keyword", query.trim());
        self.catalog(url.as_str(), ".section3 tr > td:nth-child(2) > a[href]")
    }

    fn details(&mut self, mut item: CatalogItem) -> Result<CatalogItem> {
        let (book, _) =
            book_link(&item.key).ok_or_else(|| Error::new("Invalid NovelHall book URL"))?;
        let doc = self.document(&book)?;
        item.title = first_text(&doc, ".book-info h1")?
            .ok_or_else(|| Error::new("NovelHall book details are unavailable"))?;
        item.description = first_text(&doc, ".book-info .intro")?;
        item.tags = doc
            .select(&selector(".book-info .total > a")?)
            .map(html::text)
            .map(|tag| normalize_space(&tag))
            .filter(|tag| !tag.is_empty())
            .collect();
        for value in doc.select(&selector(".book-info .total > span")?) {
            let text = value
                .children()
                .filter_map(|node| node.value().as_text())
                .map(|text| text.to_string())
                .collect::<Vec<_>>()
                .join(" ");
            if let Some(author) = text.trim().strip_prefix("Author：") {
                let author = author.trim();
                if !author.is_empty() {
                    item.authors = vec![author.to_owned()];
                }
            }
            if let Some(status) = text.trim().strip_prefix("Status：") {
                item.status = match status.trim() {
                    "Active" => Some(json!("ongoing")),
                    "Completed" => Some(json!("completed")),
                    _ => None,
                };
            }
        }
        item.cover = doc
            .select(&selector("meta[property='og:image']")?)
            .find_map(|element| attr(element, "content"))
            .map(|src| image(src, &book));
        item.content_rating = Some(rating(&item.tags).into());
        item.url = Some(book);
        item.initialized = true;
        Ok(item)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        let (book, _) =
            book_link(&item.key).ok_or_else(|| Error::new("Invalid NovelHall book URL"))?;
        let doc = self.document(&book)?;
        let mut seen = BTreeSet::new();
        let mut chapters: Vec<_> = doc
            .select(&selector("#morelist li > a[href]")?)
            .filter_map(|link| {
                let (parent, Some(url)) = book_link(link.value().attr("href")?)? else {
                    return None;
                };
                let title = normalize_space(&html::text(link));
                if parent != book || title.is_empty() || !seen.insert(url.clone()) {
                    return None;
                }
                Some(NovelChapter {
                    key: url.clone(),
                    chapter_number: number(&title),
                    title: Some(title),
                    url: Some(url),
                    ..Default::default()
                })
            })
            .collect();
        if chapters.is_empty() {
            return Err(Error::new("NovelHall chapter list is unavailable"));
        }
        chapters.reverse();
        Ok(chapters)
    }

    fn text(&mut self, _: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let (_, url) =
            book_link(&chapter.key).ok_or_else(|| Error::new("Invalid NovelHall chapter URL"))?;
        let url = url.ok_or_else(|| Error::new("Invalid NovelHall chapter URL"))?;
        chapter_text(&self.document(&url)?, "#htmlContent", &url, chapter.title)
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let Some((book, chapter)) = book_link(candidate) else {
            return Ok(None);
        };
        let item = CatalogItem::new(&book, "");
        Ok(Some(UrlResolveResult {
            item: Some(self.details(item)?),
            chapter_key: chapter,
            ..Default::default()
        }))
    }
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(manatan_sdk::Extension::new().novel(
    "novelhall",
    NovelHall {
        client: Client::browser()
    }
));
