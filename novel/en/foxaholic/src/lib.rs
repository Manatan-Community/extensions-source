use std::collections::BTreeSet;

use manatan_common::{attr, normalize_space, selector};
use manatan_sdk::{
    browser::{
        self, WebViewRequest, WebViewResponse, WebViewScript, WebViewSession, WebViewWait,
        WebViewWaitUntil,
    },
    client::{BrowserChallengePolicy, Client},
    html::{self, Html},
    model::{CatalogItem, ImageRequest, NovelChapter, NovelText, Paged, UrlResolveResult},
    storage, Error, NovelSource, Result,
};
use serde_json::Value;
use url::Url;
use wordpress_novel::{chapter_text, first_text, image_with_cookies as image, number, rating};

const BASE: &str = "https://www.foxaholic.com";
const BROWSER_PROFILE: &str = "foxaholic";
const BROWSER_USER_AGENT_KEY: &str = "browser-user-agent";

pub struct Foxaholic {
    client: Client,
}

fn book_link(candidate: &str) -> Option<(String, Option<String>)> {
    let url = Url::parse(BASE).ok()?.join(candidate).ok()?;
    if !matches!(
        url.origin().ascii_serialization().as_str(),
        BASE | "https://foxaholic.com"
    ) || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
    match parts.as_slice() {
        ["novel", slug] if !slug.is_empty() => Some((format!("{BASE}/novel/{slug}/"), None)),
        ["novel", slug, chapter] if !slug.is_empty() && !chapter.is_empty() => {
            let book = format!("{BASE}/novel/{slug}/");
            Some((book.clone(), Some(format!("{book}{chapter}/"))))
        }
        _ => None,
    }
}

impl Foxaholic {
    fn artwork(url: String, referer: &str) -> ImageRequest {
        let mut request = image(url, referer);
        if let Some(user_agent) = storage::get::<String>(BROWSER_PROFILE, BROWSER_USER_AGENT_KEY)
            .ok()
            .flatten()
            .filter(|_| {
                Url::parse(&request.url).is_ok_and(|url| url.origin().ascii_serialization() == BASE)
            })
        {
            request.headers.insert("User-Agent".into(), user_agent);
            request.cookie_url = Some(BASE.into());
        }
        request
    }

    fn document(&self, url: &str) -> Result<Html> {
        let mut request = self.client.get(url);
        if let Some(user_agent) = storage::get::<String>(BROWSER_PROFILE, BROWSER_USER_AGENT_KEY)? {
            request = request.header("User-Agent", user_agent);
        }
        // The host attaches cookies for the request origin when permitted.
        // Let the source handle the challenge so the browser can keep its
        // native user agent on the first attempt.
        let response = request.send()?;
        if !BrowserChallengePolicy::cloudflare(BASE).matches(&response) {
            return Ok(html::document(response.error_for_status()?.text()?));
        }
        // Use the platform browser's own identity and retain it for subsequent
        // requests. Cloudflare clearance is bound to the verifying user agent.
        let browser_request = WebViewRequest {
            url: url.into(),
            cookie_url: Some(BASE.into()),
            session: Some(WebViewSession {
                id: BROWSER_PROFILE.into(),
                ..Default::default()
            }),
            wait_until: Some(WebViewWaitUntil::DomReady),
            wait_for: Some(WebViewWait::Selector {
                selector: "body.wp-manga-page, .post-title, .reading-content, .post-password-form"
                    .into(),
            }),
            headers: vec![
                ("Accept".into(), "text/html,application/xhtml+xml".into()),
                ("Accept-Language".into(), "en-US,en;q=0.9".into()),
            ],
            scripts: vec![WebViewScript {
                id: Some(BROWSER_USER_AGENT_KEY.into()),
                script: "navigator.userAgent".into(),
                ..Default::default()
            }],
            timeout_ms: Some(30_000),
            return_html: true,
            ..Default::default()
        };
        let page: WebViewResponse = browser::open(&browser_request).map_err(|err| {
            Error::new(format!(
                "Foxaholic verification did not complete in Manatan's WebView: {err}"
            ))
        })?;
        if let Some(user_agent) = page
            .script_results
            .iter()
            .find(|result| result.id.as_deref() == Some(BROWSER_USER_AGENT_KEY))
            .and_then(|result| result.value.as_ref()?.as_str())
            .filter(|value| !value.is_empty() && value.len() <= 512)
        {
            storage::set(BROWSER_PROFILE, BROWSER_USER_AGENT_KEY, &user_agent)?;
        }
        let html = page
            .html
            .ok_or_else(|| Error::new("Foxaholic WebView returned no page"))?;
        Ok(html::document(&html))
    }

    fn catalog(&self, page: u32, query: Option<&str>, sort: &str) -> Result<Paged<CatalogItem>> {
        let path = match (query, page.max(1)) {
            (Some(_), 1) => "/".to_owned(),
            (Some(_), page) => format!("/page/{page}/"),
            (None, 1) => "/novel/".to_owned(),
            (None, page) => format!("/novel/page/{page}/"),
        };
        let mut url = Url::parse(BASE).unwrap().join(&path).unwrap();
        if let Some(query) = query {
            url.query_pairs_mut()
                .append_pair("s", query)
                .append_pair("post_type", "wp-manga");
        } else {
            url.query_pairs_mut().append_pair("m_orderby", sort);
        }
        let doc = self.document(url.as_str())?;
        let link = selector(".post-title a[href]")?;
        let cover = selector(".item-thumb img, .tab-thumb img")?;
        let mut seen = BTreeSet::new();
        let entries = doc
            .select(&selector(".page-item-detail, .c-tabs-item__content")?)
            .filter_map(|row| {
                let title_link = row.select(&link).next()?;
                let (book, _) = book_link(title_link.value().attr("href")?)?;
                let title = normalize_space(&html::text(title_link));
                if title.is_empty() || !seen.insert(book.clone()) {
                    return None;
                }
                let mut item = CatalogItem::new(&book, title);
                item.cover = row
                    .select(&cover)
                    .next()
                    .and_then(|img| attr(img, "data-src").or_else(|| attr(img, "src")))
                    .map(|src| Self::artwork(src, &book));
                item.url = Some(book);
                Some(item)
            })
            .collect::<Vec<_>>();
        if entries.is_empty()
            && doc
                .select(&selector(
                    "body.search-no-results, .no-results, .not-found",
                )?)
                .next()
                .is_none()
        {
            return Err(Error::new(
                "Foxaholic catalog is unavailable; check the website in WebView",
            ));
        }
        let has_next = doc
            .select(&selector(
                ".nav-previous a[href], .nav-links a.next[href], a.next.page-numbers[href]",
            )?)
            .next()
            .is_some();
        Ok(Paged::new(entries, has_next))
    }
}

impl NovelSource for Foxaholic {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.catalog(page, None, "views")
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.catalog(page, None, "latest")
    }

    fn search(&mut self, query: &str, page: u32, _: &Value) -> Result<Paged<CatalogItem>> {
        if query.trim().is_empty() {
            return self.popular(page);
        }
        self.catalog(page, Some(query.trim()), "")
    }

    fn details(&mut self, mut item: CatalogItem) -> Result<CatalogItem> {
        let (book, _) =
            book_link(&item.key).ok_or_else(|| Error::new("Invalid Foxaholic book URL"))?;
        let doc = self.document(&book)?;
        item.title = first_text(&doc, ".post-title h1")?
            .ok_or_else(|| Error::new("Foxaholic novel details are unavailable"))?;
        item.description = first_text(&doc, ".summary__content")?;
        item.tags = doc
            .select(&selector(".genres-content a, .tags-content a")?)
            .map(html::text)
            .map(|value| normalize_space(&value))
            .filter(|value| !value.is_empty())
            .collect();
        item.authors = doc
            .select(&selector(".author-content a")?)
            .map(html::text)
            .map(|value| normalize_space(&value))
            .filter(|value| !value.is_empty())
            .collect();
        item.cover = doc
            .select(&selector(".summary_image img")?)
            .next()
            .and_then(|img| attr(img, "data-src").or_else(|| attr(img, "src")))
            .map(|src| Self::artwork(src, &book))
            .or(item.cover);
        item.content_rating = Some(rating(&item.tags).into());
        item.url = Some(book);
        item.initialized = true;
        Ok(item)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        let (book, _) =
            book_link(&item.key).ok_or_else(|| Error::new("Invalid Foxaholic book URL"))?;
        let doc = self.document(&book)?;
        let link_selector = selector("a[href]")?;
        let mut seen = BTreeSet::new();
        let chapters = doc
            .select(&selector("li.wp-manga-chapter")?)
            .filter_map(|row| {
                let link = row.select(&link_selector).next()?;
                let href = link.value().attr("href")?;
                let is_locked = row.value().classes().any(|class| {
                    matches!(
                        class,
                        "locked"
                            | "locked-chap"
                            | "paid-chap"
                            | "premium-chap"
                            | "premium"
                            | "premium-block"
                    )
                });
                let (key, url) = if href == "#" && is_locked {
                    let id = row
                        .value()
                        .classes()
                        .find_map(|class| class.strip_prefix("data-chapter-"))
                        .filter(|id| {
                            !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())
                        })?;
                    (format!("{book}#chapter-{id}"), None)
                } else {
                    let (parent, Some(url)) = book_link(href)? else {
                        return None;
                    };
                    if parent != book {
                        return None;
                    }
                    (url.clone(), Some(url))
                };
                let title = normalize_space(&html::text(link));
                if title.is_empty() || !seen.insert(key.clone()) {
                    return None;
                }
                Some(NovelChapter {
                    key,
                    chapter_number: number(&title),
                    title: Some(title),
                    url,
                    is_locked,
                    ..Default::default()
                })
            })
            .collect::<Vec<_>>();
        if chapters.is_empty() {
            return Err(Error::new("Foxaholic chapters are unavailable; check this novel's access requirements on the website"));
        }
        Ok(chapters)
    }

    fn text(&mut self, _: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        if chapter.is_locked {
            return Err(Error::new("This Foxaholic chapter is locked. Open the novel on Foxaholic to check its access requirements."));
        }
        let (_, url) =
            book_link(&chapter.key).ok_or_else(|| Error::new("Invalid Foxaholic chapter URL"))?;
        let url = url.ok_or_else(|| Error::new("Invalid Foxaholic chapter URL"))?;
        let doc = self.document(&url)?;
        if doc
            .select(&selector(
                ".post-password-form, .reading-content .locked-content",
            )?)
            .next()
            .is_some()
        {
            return Err(Error::new("This Foxaholic chapter is locked. Open it on Foxaholic to check its access requirements."));
        }
        chapter_text(&doc, ".reading-content .text-left", &url, chapter.title)
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
    "foxaholic",
    Foxaholic {
        client: Client::browser()
    }
));
