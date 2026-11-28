use manatan_common::{normalize_space, selector};
use manatan_sdk::{
    client::{Client, Response},
    html::{self, Html},
    model::{CatalogItem, NovelChapter, NovelText, Paged, UrlResolveResult},
    Error, NovelSource, Result,
};
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;
use wordpress_novel::{chapter_text, first_text, image, number, rating};

const BASE: &str = "https://chrysanthemumgarden.org";
const LEGACY_BASE: &str = "https://chrysanthemumgarden.com";
const PAGE_SIZE: usize = 20;

pub struct ChrysanthemumGarden {
    client: Client,
}

struct Link {
    book: String,
    chapter: Option<String>,
}

impl Link {
    fn parse(candidate: &str) -> Option<Self> {
        let url = Url::parse(BASE).ok()?.join(candidate).ok()?;
        if !matches!(
            url.origin().ascii_serialization().as_str(),
            BASE | LEGACY_BASE
        ) || !url.username().is_empty()
            || url.password().is_some()
        {
            return None;
        }
        let mut path = url
            .path()
            .trim_end_matches('/')
            .strip_prefix("/novel-tl/")?
            .split('/');
        let book = path.next().filter(|s| slug(s))?.to_owned();
        let chapter = match (path.next(), path.next()) {
            (None, None) => None,
            (Some(value), None) if slug(value) => Some(value.to_owned()),
            _ => return None,
        };
        Some(Self { book, chapter })
    }

    fn book_url(&self, base: &str) -> String {
        format!("{base}/novel-tl/{}/", self.book)
    }

    fn chapter_url(&self, base: &str) -> Option<String> {
        Some(format!(
            "{}{}/",
            self.book_url(base),
            self.chapter.as_deref()?
        ))
    }

    fn item(&self, title: impl Into<String>, cover: Option<&str>) -> CatalogItem {
        // Existing library and progress records use the old canonical URLs as keys.
        let mut item = CatalogItem::new(self.book_url(LEGACY_BASE), title);
        let url = self.book_url(BASE);
        item.cover = cover
            .and_then(|src| Url::parse(&url).ok()?.join(src).ok())
            .filter(|cover| {
                cover.origin().ascii_serialization() == BASE
                    && cover.username().is_empty()
                    && cover.password().is_none()
            })
            .map(|cover| image(cover.into(), &url));
        item.url = Some(url);
        item
    }
}

fn slug(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Book {
    slug: String,
    title: String,
    cover_local: Option<String>,
    original_title: Option<String>,
    author: Option<String>,
    synopsis: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    chapters: Vec<Chapter>,
}

#[derive(Deserialize)]
struct Chapter {
    slug: String,
    title: String,
    archived: bool,
}

impl Book {
    fn matches(&self, query: &str) -> bool {
        [
            Some(self.title.as_str()),
            self.original_title.as_deref(),
            self.author.as_deref(),
            self.synopsis.as_deref(),
        ]
        .into_iter()
        .flatten()
        .chain(self.genres.iter().chain(&self.tags).map(String::as_str))
        .any(|value| value.to_lowercase().contains(query))
    }

    fn item(&self) -> Option<CatalogItem> {
        if !slug(&self.slug) || self.title.trim().is_empty() {
            return None;
        }
        let link = Link {
            book: self.slug.clone(),
            chapter: None,
        };
        Some(link.item(self.title.clone(), self.cover_local.as_deref()))
    }

    fn readable_chapters(&self) -> Vec<NovelChapter> {
        // Manatan expects chapters in newest-first order.
        self.chapters
            .iter()
            .rev()
            .filter(|chapter| chapter.archived && slug(&chapter.slug))
            .filter_map(|chapter| {
                let link = Link {
                    book: self.slug.clone(),
                    chapter: Some(chapter.slug.clone()),
                };
                Some(NovelChapter {
                    key: link.chapter_url(LEGACY_BASE)?,
                    title: Some(chapter.title.clone()),
                    chapter_number: number(&chapter.title),
                    url: link.chapter_url(BASE),
                    language: Some("en".into()),
                    ..Default::default()
                })
            })
            .collect()
    }
}

fn catalog(doc: &Html, links: &str) -> Result<Vec<CatalogItem>> {
    let image_selector = selector("img")?;
    Ok(doc
        .select(&selector(links)?)
        .filter_map(|anchor| {
            let link = Link::parse(anchor.value().attr("href")?)?;
            let title = normalize_space(anchor.value().attr("aria-label")?);
            if title.is_empty() || link.chapter.is_some() {
                return None;
            }
            let cover = anchor
                .select(&image_selector)
                .next()
                .and_then(|img| img.value().attr("src"));
            Some(link.item(title, cover))
        })
        .collect())
}

fn paginate(entries: impl Iterator<Item = CatalogItem>, page: u32) -> Paged<CatalogItem> {
    let offset = (page.saturating_sub(1) as usize).saturating_mul(PAGE_SIZE);
    let mut page_entries: Vec<_> = entries.skip(offset).take(PAGE_SIZE + 1).collect();
    let has_next = page_entries.len() > PAGE_SIZE;
    page_entries.truncate(PAGE_SIZE);
    Paged::new(page_entries, has_next)
}

impl ChrysanthemumGarden {
    fn get(&self, url: &str) -> Result<Response> {
        self.client.get(url).send()?.error_for_status()
    }

    fn document(&self, url: &str) -> Result<Html> {
        Ok(html::document(self.get(url)?.text()?))
    }

    fn book(&self, link: &Link) -> Result<Book> {
        let book: Book = self
            .get(&format!("{BASE}/data/books/{}.json", link.book))?
            .json()?;
        if book.slug != link.book {
            return Err(Error::new("The archive returned a different novel"));
        }
        Ok(book)
    }
}

impl NovelSource for ChrysanthemumGarden {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let url = if page <= 1 {
            format!("{BASE}/books/")
        } else {
            format!("{BASE}/books/page/{page}/")
        };
        let doc = self.document(&url)?;
        let entries = catalog(&doc, "#book-list .book-entry a.book-cover")?;
        let has_next = doc.select(&selector("link[rel=next]")?).next().is_some();
        Ok(Paged::new(entries, has_next))
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let doc = self.document(&format!("{BASE}/new/"))?;
        let entries = catalog(&doc, ".novel-shelf .novel-card a.novel-card-cover")?;
        Ok(paginate(entries.into_iter(), page))
    }

    fn search(&mut self, query: &str, page: u32, _: &Value) -> Result<Paged<CatalogItem>> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return self.popular(page);
        }
        // The archive applies searches in the browser against this public catalog.
        let books: Vec<Book> = self.get(&format!("{BASE}/data/catalog.json"))?.json()?;
        Ok(paginate(
            books
                .iter()
                .filter(|book| book.matches(&query))
                .filter_map(Book::item),
            page,
        ))
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let link = Link::parse(&item.key)
            .ok_or_else(|| Error::new("Invalid Chrysanthemum Garden novel URL"))?;
        let book = self.book(&link)?;
        let mut details = book
            .item()
            .ok_or_else(|| Error::new("The archive returned incomplete novel details"))?;
        details.key = item.key;
        details.description = book.synopsis.filter(|s| !s.trim().is_empty());
        details.authors = book
            .author
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .collect();
        details.tags = book.genres;
        for tag in book.tags {
            // Some archived tags include the old site's count in parentheses.
            let tag = tag
                .rsplit_once(" (")
                .filter(|(_, count)| {
                    count.strip_suffix(')').is_some_and(|count| {
                        !count.is_empty() && count.bytes().all(|b| b.is_ascii_digit())
                    })
                })
                .map_or(tag.as_str(), |(label, _)| label);
            if !tag.is_empty()
                && !details
                    .tags
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(tag))
            {
                details.tags.push(tag.to_owned());
            }
        }
        if details
            .tags
            .iter()
            .any(|tag| tag.eq_ignore_ascii_case("completed"))
        {
            details.status = Some(json!("completed"));
        }
        details.content_rating = Some(
            if rating(&details.tags) == "adult" {
                "adult"
            } else {
                "suggestive"
            }
            .into(),
        );
        details.initialized = true;
        Ok(details)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        let link = Link::parse(&item.key)
            .ok_or_else(|| Error::new("Invalid Chrysanthemum Garden novel URL"))?;
        Ok(self.book(&link)?.readable_chapters())
    }

    fn text(&mut self, _: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let url = Link::parse(&chapter.key)
            .and_then(|link| link.chapter_url(BASE))
            .ok_or_else(|| Error::new("Invalid Chrysanthemum Garden chapter URL"))?;
        let doc = self.document(&url)?;
        let reader = ".reader-shell[data-ssr-readable='1'] .reader-paper";
        let title = match chapter.title {
            Some(title) => Some(title),
            None => first_text(&doc, ".reader-paper-header h1")?,
        };
        chapter_text(&doc, &format!("{reader} .reader-chapter-text"), &url, title)
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let Some(link) = Link::parse(candidate) else {
            return Ok(None);
        };
        Ok(Some(UrlResolveResult {
            item: Some(self.details(link.item("", None))?),
            chapter_key: link.chapter_url(LEGACY_BASE),
            ..Default::default()
        }))
    }
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(manatan_sdk::Extension::new().novel(
    "chrysanthemumgarden",
    ChrysanthemumGarden {
        client: Client::new()
    }
));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_cards_keep_legacy_ids_and_publish_live_links() {
        let doc = html::document(
            r#"<section id="book-list"><article class="book-entry">
                <a class="book-cover" href="/novel-tl/example/" aria-label="Example &amp; Co">
                    <img src="/covers/example.webp">
                </a>
            </article></section>"#,
        );
        let entries = catalog(&doc, "#book-list .book-entry a.book-cover").unwrap();
        let item = &entries[0];
        assert_eq!(item.title, "Example & Co");
        assert_eq!(item.key, format!("{LEGACY_BASE}/novel-tl/example/"));
        assert_eq!(
            item.url.as_deref(),
            Some("https://chrysanthemumgarden.org/novel-tl/example/")
        );
        assert_eq!(
            item.cover.as_ref().unwrap().url,
            format!("{BASE}/covers/example.webp")
        );
    }

    #[test]
    fn book_index_keeps_every_archived_english_chapter_in_reader_order() {
        let source = r#"{
            "slug": "example", "title": "Example", "author": "Author Name",
            "genres": ["Fantasy"], "tags": ["Found Family"],
            "chapters": [
                {"slug": "chapter-1", "title": "Chapter 1", "archived": true},
                {"slug": "chapter-2", "title": "Chapter 2", "archived": false},
                {"slug": "chapter-3", "title": "Chapter 3", "archived": true}
            ]
        }"#;
        let book: Book = serde_json::from_str(source).unwrap();
        assert!(book.matches("author name"));
        assert!(book.matches("found family"));
        assert!(!book.matches("no matching title"));
        let chapters = book.readable_chapters();
        assert_eq!(chapters.len(), 2);
        assert_eq!(
            chapters[0].key,
            format!("{LEGACY_BASE}/novel-tl/example/chapter-3/")
        );
        assert_eq!(chapters[1].chapter_number, Some(1.0));
    }

    #[test]
    fn chapter_deep_links_accept_both_domains_without_following_other_hosts() {
        for origin in [LEGACY_BASE, BASE] {
            let link =
                Link::parse(&format!("{origin}/novel-tl/example/ch-1/?ref=share#start")).unwrap();
            assert_eq!(
                link.chapter_url(BASE).unwrap(),
                format!("{BASE}/novel-tl/example/ch-1/")
            );
        }
        for url in [
            "https://chrysanthemumgarden.org.example.com/novel-tl/example/",
            "https://other@chrysanthemumgarden.org/novel-tl/example/",
            "https://chrysanthemumgarden.org/novel-tl/example/ch-1/extra/",
        ] {
            assert!(Link::parse(url).is_none());
        }
    }
}
