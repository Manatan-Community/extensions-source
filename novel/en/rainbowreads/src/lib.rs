use manatan_common::{absolute_url, attr, normalize_space, selector};
use manatan_sdk::{
    client::{BrowserChallengePolicy, Client},
    html::{self, Html},
    model::{CatalogItem, NovelChapter, NovelText, Paged, UrlResolveResult},
    Error, NovelSource, Result,
};
use serde_json::{json, Value};
use url::Url;
use wordpress_novel::{chapter_text, first_text, image, number, rating};
const BASE: &str = "https://rainbow-reads.com";
const READER_CONTENT: &str = ".epcontent.entry-content, .elementor[data-elementor-type='wp-page']";
pub struct Rainbow {
    client: Client,
}
impl Default for Rainbow {
    fn default() -> Self {
        Self {
            client: Client::browser().cookies_for(BASE),
        }
    }
}
impl Rainbow {
    fn document(&self, url: &str) -> Result<Html> {
        let parsed = Url::parse(url).map_err(|_| Error::new("Invalid Rainbow Reads URL"))?;
        if parsed.scheme() != "https" || parsed.host_str() != Some("rainbow-reads.com") {
            return Err(Error::new("Not a Rainbow Reads URL"));
        }
        let response = self
            .client
            .get(url)
            .send_with_challenge(&BrowserChallengePolicy::cloudflare(BASE).profile("rainbowreads"))?
            .error_for_status()?;
        Ok(html::document(response.text()?))
    }
    fn catalog_url(query: &str, page: u32, latest: bool) -> Result<String> {
        let page = page.max(1);
        let path = if query.is_empty() {
            if page == 1 {
                "/series/".into()
            } else {
                format!("/series/page/{page}/")
            }
        } else if page == 1 {
            "/".into()
        } else {
            format!("/page/{page}/")
        };
        let mut url =
            Url::parse(&format!("{BASE}{path}")).map_err(|_| Error::new("Invalid catalog URL"))?;
        if query.is_empty() {
            url.query_pairs_mut()
                .append_pair("order", if latest { "update" } else { "popular" });
        } else {
            url.query_pairs_mut().append_pair("s", query);
        }
        Ok(url.to_string())
    }
    fn catalog(doc: &Html) -> Result<Paged<CatalogItem>> {
        let mut entries = Vec::new();
        for card in doc.select(&selector("article.maindet")?) {
            let link = card
                .select(&selector(".mdinfo h2 a")?)
                .next()
                .ok_or_else(|| Error::new("Rainbow Reads catalog title is missing"))?;
            let url = absolute_url(
                BASE,
                &attr(link, "href")
                    .ok_or_else(|| Error::new("Rainbow Reads novel URL is missing"))?,
            )?;
            let title = normalize_space(&html::text(link));
            let mut item = CatalogItem::new(&url, title);
            item.url = Some(url.clone());
            item.language = Some("en".into());
            item.cover = card
                .select(&selector(".mdthumb img")?)
                .next()
                .and_then(|n| attr(n, "data-src").or_else(|| attr(n, "src")))
                .map(|s| absolute_url(BASE, &s))
                .transpose()?
                .map(|s| image(s, &url));
            item.description = card
                .select(&selector(".contexcerpt")?)
                .next()
                .map(|n| normalize_space(&html::text(n)));
            item.tags = card
                .select(&selector(".mdgenre a")?)
                .map(html::text)
                .map(|s| normalize_space(s.trim_start_matches('#')))
                .collect();
            item.content_rating = Some(rating(&item.tags).into());
            entries.push(item);
        }
        if entries.is_empty()
            && doc
                .select(&selector(".listupd, .notf, .notfound, .search-results")?)
                .next()
                .is_none()
        {
            return Err(Error::new("Rainbow Reads catalog markup is missing"));
        }
        let next = doc
            .select(&selector(".hpage a, a.next.page-numbers, link[rel=next]")?)
            .any(|n| {
                attr(n, "rel").as_deref() == Some("next")
                    || normalize_space(&html::text(n))
                        .to_lowercase()
                        .contains("next")
            });
        Ok(Paged::new(entries, next))
    }
    fn browse(&self, q: &str, page: u32, latest: bool) -> Result<Paged<CatalogItem>> {
        Self::catalog(&self.document(&Self::catalog_url(q, page, latest)?)?)
    }
    fn detail(doc: &Html, url: &str) -> Result<CatalogItem> {
        let title = first_text(doc, "h1.entry-title")?
            .ok_or_else(|| Error::new("Rainbow Reads novel title is missing"))?;
        let mut i = CatalogItem::new(url, title);
        i.url = Some(url.into());
        i.language = Some("en".into());
        i.initialized = true;
        i.description = first_text(doc, ".sersys.entry-content")?;
        i.cover = doc
            .select(&selector(".sertothumb img, .sertoimg img, .sertobig img")?)
            .next()
            .and_then(|n| attr(n, "data-src").or_else(|| attr(n, "src")))
            .or_else(|| {
                doc.select(&selector("meta[property='og:image']").ok()?)
                    .next()
                    .and_then(|n| attr(n, "content"))
            })
            .map(|s| absolute_url(BASE, &s))
            .transpose()?
            .map(|s| image(s, url));
        i.tags = doc
            .select(&selector(".sertogenre a, .sertoinfo a[href*='/genre/']")?)
            .map(html::text)
            .map(|s| normalize_space(&s))
            .collect();
        i.content_rating = Some(rating(&i.tags).into());
        i.authors = doc
            .select(&selector(".sertoauth .serl")?)
            .filter_map(|n| {
                let label = n
                    .select(&selector(".sername").ok()?)
                    .next()
                    .map(html::text)?;
                if label.trim() != "Author" {
                    return None;
                }
                n.select(&selector(".serval").ok()?)
                    .next()
                    .map(html::text)
                    .map(|s| normalize_space(&s))
            })
            .collect();
        i.status = Some(json!(match first_text(doc, ".sertostat")?
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "completed" | "complete" => "completed",
            "ongoing" => "ongoing",
            "dropped" => "cancelled",
            _ => "unknown",
        }));
        Ok(i)
    }
    fn chapter_rows(doc: &Html) -> Result<Vec<NovelChapter>> {
        if doc.select(&selector(".eplister")?).next().is_none() {
            return Err(Error::new("Rainbow Reads chapter index is missing"));
        }
        let mut chapters = Vec::new();
        for a in doc.select(&selector(".eplister ul li a")?) {
            let url = absolute_url(
                BASE,
                &attr(a, "href")
                    .ok_or_else(|| Error::new("Rainbow Reads chapter URL is missing"))?,
            )?;
            let title = a
                .select(&selector(".epl-title")?)
                .next()
                .map(html::text)
                .map(|s| normalize_space(&s))
                .unwrap_or_else(|| normalize_space(&html::text(a)));
            let n = a
                .select(&selector(".epl-num")?)
                .next()
                .map(html::text)
                .and_then(|s| number(&s))
                .or_else(|| number(&title));
            chapters.push(NovelChapter {
                key: url.clone(),
                url: Some(url),
                title: Some(title),
                chapter_number: n,
                language: Some("en".into()),
                ..NovelChapter::default()
            });
        }
        // The site publishes oldest-first, including numbered parts and extras.
        // Reverse source order instead of collapsing/sorting duplicate numbers.
        chapters.reverse();
        Ok(chapters)
    }
}
impl NovelSource for Rainbow {
    fn popular(&mut self, p: u32) -> Result<Paged<CatalogItem>> {
        self.browse("", p, false)
    }
    fn latest(&mut self, p: u32) -> Result<Paged<CatalogItem>> {
        self.browse("", p, true)
    }
    fn search(&mut self, q: &str, p: u32, _: &Value) -> Result<Paged<CatalogItem>> {
        self.browse(q, p, false)
    }
    fn details(&mut self, i: CatalogItem) -> Result<CatalogItem> {
        let url = i.url.as_deref().unwrap_or(&i.key);
        Self::detail(&self.document(url)?, url)
    }
    fn chapters(&mut self, i: CatalogItem) -> Result<Vec<NovelChapter>> {
        Self::chapter_rows(&self.document(i.url.as_deref().unwrap_or(&i.key))?)
    }
    fn text(&mut self, _: CatalogItem, c: NovelChapter) -> Result<NovelText> {
        if c.is_locked {
            return Err(Error::new(
                "This Rainbow Reads chapter is locked. Open it on the website to unlock it.",
            ));
        }
        let url = c.url.as_deref().unwrap_or(&c.key);
        let doc = self.document(url)?;
        if doc
            .select(&selector(".post-password-form")?)
            .next()
            .is_some()
        {
            return Err(Error::new("This Rainbow Reads chapter is password-protected. Open the website and enter the author's password."));
        }
        chapter_text(&doc, READER_CONTENT, url, c.title)
    }
    fn handle_url(&mut self, url: &str) -> Result<Option<UrlResolveResult>> {
        let Ok(p) = Url::parse(url) else {
            return Ok(None);
        };
        if p.scheme() != "https" || p.host_str() != Some("rainbow-reads.com") {
            return Ok(None);
        }
        let doc = self.document(url)?;
        if doc.select(&selector(".eplister")?).next().is_some() {
            return Ok(Some(UrlResolveResult {
                item: Some(Self::detail(&doc, url)?),
                ..Default::default()
            }));
        }
        if let Some(a) = doc.select(&selector(".naveps a[href]")?).find(|a| {
            normalize_space(&html::text(*a))
                .to_lowercase()
                .contains("all chapter")
        }) {
            if let Some(href) = attr(a, "href") {
                let target = absolute_url(BASE, &href)?;
                let mut i = CatalogItem::new(&target, "");
                i.url = Some(target);
                return Ok(Some(UrlResolveResult {
                    item: Some(i),
                    ..Default::default()
                }));
            }
        }
        Ok(None)
    }
}
#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel("rainbowreads", Rainbow::default())
}
#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_chapters_keep_source_order_and_same_number() {
        let doc=html::document("<div class=eplister><ul><li><a href='/ch-1a/'><div class=epl-num>Ch. 1</div><div class=epl-title>Chapter 1 Part 1</div></a></li><li><a href='/ch-1b/'><div class=epl-num>Ch. 1</div><div class=epl-title>Chapter 1 Part 2</div></a></li></ul></div>");
        let c = Rainbow::chapter_rows(&doc).unwrap();
        assert_eq!(c.len(), 2);
        assert!(c[0].key.ends_with("ch-1b/"));
        assert_eq!(c[0].chapter_number, c[1].chapter_number);
    }
    #[test]
    fn encoded_search_and_real_pagination_paths() {
        let url = Rainbow::catalog_url("A & B", 2, false).unwrap();
        assert!(url.contains("/page/2/"));
        assert!(url.contains("s=A+%26+B"));
        assert!(Rainbow::catalog_url("", 2, true)
            .unwrap()
            .contains("/series/page/2/?order=update"));
    }
    #[test]
    fn pagination_and_boys_love_not_adult() {
        let doc=html::document("<div class=listupd><article class=maindet><div class=mdinfo><h2><a href='/test/'>Test</a></h2><div class=mdgenre><a># Boy's Love</a></div></div></article></div><div class=hpage><a href='/series/page/2/'>Next</a></div>");
        let page = Rainbow::catalog(&doc).unwrap();
        assert!(page.has_next_page);
        assert_eq!(page.entries[0].content_rating.as_deref(), Some("general"));
    }
    #[test]
    fn missing_markup_is_not_an_empty_index() {
        assert!(Rainbow::chapter_rows(&html::document("Unavailable")).is_err());
    }
    #[test]
    fn supplemental_elementor_pages_use_only_the_public_page_content() {
        let doc = html::document("<header>Navigation</header><div data-elementor-type='wp-page' class=elementor><p>Supplemental artwork notes.</p><img src='/art.jpg'><script>unsafe()</script></div><footer>Unrelated links</footer>");
        let text = chapter_text(&doc, READER_CONTENT, BASE, Some("Extra".into())).unwrap();
        let html = text.html.unwrap();
        assert!(html.contains("Supplemental artwork notes."));
        assert!(html.contains("art.jpg"));
        assert!(
            !html.contains("Navigation") && !html.contains("Unrelated") && !html.contains("unsafe")
        );
    }
}
