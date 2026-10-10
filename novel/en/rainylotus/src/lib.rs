//! Original Lotus-theme source. Only public HTML is parsed; page scripts never run.
use std::collections::BTreeSet;

use manatan_common::{absolute_url, attr, normalize_space, selector};
use manatan_sdk::{
    browser::{self, WebViewRequest, WebViewSession},
    client::{BrowserChallengePolicy, Client},
    html::{self, Html},
    model::{
        AuthenticationAction, AuthenticationRequest, AuthenticationState,
        AuthenticationWebViewCompletion, CatalogItem, FilterDefinition, NovelChapter,
        NovelChapterPage, NovelText, OptionItem, Paged, UrlResolveResult,
    },
    Error, NovelSource, Result,
};
use serde_json::{json, Value};
use url::Url;
use wordpress_novel::{chapter_text, first_text, image, number, rating};

const BASE: &str = "https://rainylotus.com";
const PROFILE: &str = "rainylotus";

pub struct RainyLotus {
    client: Client,
}

impl Default for RainyLotus {
    fn default() -> Self {
        Self {
            client: Client::browser().cookies_for(BASE),
        }
    }
}

fn site_url(candidate: &str) -> Result<Url> {
    let url = Url::parse(candidate).map_err(|_| Error::new("Invalid Rainy Lotus URL"))?;
    if url.scheme() != "https"
        || url.host_str() != Some("rainylotus.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err(Error::new("Not a Rainy Lotus URL"));
    }
    Ok(url)
}

/// Strip reader paths and tracking parameters, keeping catalog identity stable.
fn novel_url(candidate: &str) -> Result<String> {
    let url = site_url(candidate)?;
    let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
    if !(2..=3).contains(&parts.len())
        || parts[0] != "novel"
        || parts[1].is_empty()
        || (parts.len() == 3 && parts[2].is_empty())
    {
        return Err(Error::new("Rainy Lotus novel URL is missing"));
    }
    Ok(format!("{BASE}/novel/{}", parts[1]))
}

fn content_rating(tags: &[String]) -> &'static str {
    if rating(tags) == "adult" || tags.iter().any(|t| t.eq_ignore_ascii_case("R-18")) {
        "adult"
    } else if tags.iter().any(|t| t.eq_ignore_ascii_case("Mature")) {
        "suggestive"
    } else {
        "general"
    }
}

fn login_action() -> AuthenticationState {
    AuthenticationState {
        message: Some("Sign in on Rainy Lotus. Paid chapters must be unlocked on its website; the source never spends Lotus or Cookies automatically.".into()),
        action: Some(AuthenticationAction::WebView {
            url: format!("{BASE}/login"),
            profile: PROFILE.into(),
            cookie_url: Some(BASE.into()),
            completion: AuthenticationWebViewCompletion::default(),
        }),
        ..Default::default()
    }
}

impl RainyLotus {
    fn document(&self, url: &str) -> Result<Html> {
        site_url(url)?;
        let response = self
            .client
            .get(url)
            .send_with_challenge(&BrowserChallengePolicy::cloudflare(BASE).profile(PROFILE))?
            .error_for_status()?;
        site_url(response.final_url())?;
        Ok(html::document(response.text()?))
    }

    fn catalog_url(query: &str, page: u32, popular: bool, filters: &Value) -> Result<String> {
        let page = page.max(1);
        let path = if page == 1 {
            "/series/".into()
        } else {
            format!("/series/page/{page}/")
        };
        let mut url = site_url(&format!("{BASE}{path}"))?;
        if !query.trim().is_empty() {
            url.query_pairs_mut().append_pair("q", query.trim());
        }
        let sort = filters
            .get("sort")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(if popular { "views" } else { "" });
        if !["", "rating", "views", "oldest", "title", "chapters"].contains(&sort) {
            return Err(Error::new("Unsupported Rainy Lotus sort order"));
        }
        if !sort.is_empty() {
            url.query_pairs_mut().append_pair("sort", sort);
        }
        for key in ["genre", "tag", "status"] {
            let Some(value) = filters
                .get(key)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
            else {
                continue;
            };
            if !value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                || (key == "status" && !["ongoing", "completed"].contains(&value))
            {
                return Err(Error::new("Invalid Rainy Lotus filter value"));
            }
            url.query_pairs_mut().append_pair(key, value);
        }
        Ok(url.into())
    }

    fn catalog(doc: &Html) -> Result<Paged<CatalogItem>> {
        // Missing results is a schema failure, not an empty successful search.
        if doc
            .select(&selector("#ac-archive-results")?)
            .next()
            .is_none()
        {
            return Err(Error::new("Rainy Lotus catalog markup is missing"));
        }
        let mut entries = Vec::new();
        let mut seen = BTreeSet::new();
        for card in doc.select(&selector("#ac-archive-results article.ac-grid2-card")?) {
            let link = card
                .select(&selector("a.ac-grid2-title-link")?)
                .next()
                .ok_or_else(|| Error::new("Rainy Lotus catalog title link is missing"))?;
            let title = normalize_space(&html::text(link));
            if title.is_empty() {
                return Err(Error::new("Rainy Lotus catalog title is missing"));
            }
            let url = novel_url(&absolute_url(
                BASE,
                &attr(link, "href")
                    .ok_or_else(|| Error::new("Rainy Lotus catalog novel URL is missing"))?,
            )?)?;
            if !seen.insert(url.clone()) {
                continue;
            }
            let mut item = CatalogItem::new(&url, title);
            item.url = Some(url.clone());
            item.language = Some("en".into());
            item.cover = card
                .select(&selector("img.ac-grid2-cover-img")?)
                .next()
                .and_then(|n| attr(n, "data-src").or_else(|| attr(n, "src")))
                .map(|s| absolute_url(BASE, &s))
                .transpose()?
                .map(|s| image(s, &url));
            item.tags = card
                .select(&selector(".ac-grid2-genres a")?)
                .map(html::text)
                .map(|s| normalize_space(s.trim_end_matches(',')))
                .collect();
            item.content_rating = Some(content_rating(&item.tags).into());
            entries.push(item);
        }
        let next = doc
            .select(&selector("#ac-archive-pagination a.next.page-numbers")?)
            .next()
            .is_some();
        Ok(Paged::new(entries, next))
    }

    fn browse(
        &self,
        query: &str,
        page: u32,
        popular: bool,
        filters: &Value,
    ) -> Result<Paged<CatalogItem>> {
        Self::catalog(&self.document(&Self::catalog_url(query, page, popular, filters)?)?)
    }

    fn detail(doc: &Html, candidate: &str) -> Result<CatalogItem> {
        let url = novel_url(candidate)?;
        let title = first_text(doc, "h1#ac-novel-title")?
            .ok_or_else(|| Error::new("Rainy Lotus novel title is missing"))?;
        let mut item = CatalogItem::new(&url, title);
        item.url = Some(url.clone());
        item.initialized = true;
        // The original novel may be Chinese/Korean; the reading language is English.
        item.language = Some("en".into());
        item.description = first_text(doc, "#synopsis-content")?;
        item.cover = doc
            .select(&selector("#ac-cover-img > img")?)
            .next()
            .and_then(|n| attr(n, "data-src").or_else(|| attr(n, "src")))
            .map(|s| absolute_url(BASE, &s))
            .transpose()?
            .map(|s| image(s, &url));
        let mut seen = BTreeSet::new();
        item.tags = doc
            .select(&selector("#ac-genres-row a, #ac-tags-row a")?)
            .map(html::text)
            .map(|s| normalize_space(&s))
            .filter(|s| !s.is_empty() && seen.insert(s.clone()))
            .collect();
        item.content_rating = Some(content_rating(&item.tags).into());
        item.authors = first_text(doc, "#ac-meta-author .ac-meta-chip")?
            .into_iter()
            .collect();
        item.status = Some(json!(
            match first_text(doc, "#ac-meta-status .ac-meta-chip")?
                .unwrap_or_default()
                .to_lowercase()
                .as_str()
            {
                "completed" => "completed",
                "ongoing" => "ongoing",
                _ => "unknown",
            }
        ));
        if let Some(origin) = first_text(doc, "#ac-meta-origin .ac-meta-chip")? {
            item.extra.insert("originalLanguage".into(), json!(origin));
        }
        Ok(item)
    }

    fn chapter_rows(doc: &Html, candidate: &str) -> Result<Vec<NovelChapter>> {
        let root = novel_url(candidate)?;
        if doc.select(&selector("#chapters")?).next().is_none() {
            return Err(Error::new("Rainy Lotus chapter index is missing"));
        }
        let mut seen = BTreeSet::new();
        let mut chapters = Vec::new();
        for row in doc.select(&selector("#chapters .chapter-row")?) {
            let link = row
                .select(&selector("a.chapter-el")?)
                .next()
                .ok_or_else(|| Error::new("Rainy Lotus chapter link is missing"))?;
            let mut url = site_url(&absolute_url(
                BASE,
                &attr(link, "href")
                    .ok_or_else(|| Error::new("Rainy Lotus chapter URL is missing"))?,
            )?)?;
            url.set_query(None);
            url.set_fragment(None);
            if novel_url(url.as_str())? != root
                || !url
                    .path()
                    .starts_with(&format!("{}/", Url::parse(&root).unwrap().path()))
            {
                return Err(Error::new(
                    "Rainy Lotus chapter belongs to a different novel",
                ));
            }
            let url = url.to_string();
            if !seen.insert(url.clone()) {
                continue;
            }
            let label = attr(link, "data-ac-chapter-label")
                .or_else(|| attr(row, "data-ac-reading-label"))
                .unwrap_or_else(|| normalize_space(&html::text(link)));
            let locked = match attr(link, "data-ac-locked").as_deref() {
                Some("1") => true,
                Some("0") => false,
                // The server omits the unlock-dialog attributes entirely for
                // public chapters. Paid rows carry an explicit flag and icon.
                None if link
                    .select(&selector("[data-ac-access-icon=lock]")?)
                    .next()
                    .is_none()
                    && attr(link, "data-ac-cost").is_none() =>
                {
                    false
                }
                _ => return Err(Error::new("Rainy Lotus chapter access metadata is missing")),
            };
            chapters.push(NovelChapter {
                key: url.clone(),
                url: Some(url),
                title: Some(label.clone()),
                chapter_number: attr(row, "data-num")
                    .and_then(|n| n.parse::<f32>().ok())
                    .filter(|n| n.is_finite())
                    .or_else(|| number(&label)),
                language: Some("en".into()),
                is_locked: locked,
                ..Default::default()
            });
        }
        // Already newest-first on the website. Preserve parts/extras and duplicate
        // chapter numbers. Newer site versions return only one page here.
        Ok(chapters)
    }

    fn chapter_page_count(doc: &Html) -> Result<u32> {
        let Some(label) = first_text(doc, "#ac-chapter-page-label")? else {
            return Ok(1); // Older Lotus versions render the complete index.
        };
        let (page, pages) = label
            .split_once('/')
            .ok_or_else(|| Error::new("Rainy Lotus chapter pagination is invalid"))?;
        let pages = pages
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=2000).contains(n));
        if page.trim() != "1" || pages.is_none() {
            return Err(Error::new("Rainy Lotus chapter pagination is invalid"));
        }
        Ok(pages.unwrap())
    }

    fn chapter_request_metadata(doc: &Html) -> Result<(String, String)> {
        let id = doc
            .select(&selector("body")?)
            .next()
            .and_then(|body| attr(body, "class"))
            .and_then(|classes| {
                classes
                    .split_whitespace()
                    .find_map(|c| c.strip_prefix("postid-").map(str::to_owned))
            })
            .filter(|id| !id.is_empty() && id.bytes().all(|c| c.is_ascii_digit()))
            .ok_or_else(|| Error::new("Rainy Lotus chapter catalog ID is missing"))?;
        // Read a JSON data declaration, never execute site scripts or trust a
        // script-provided endpoint. This public nonce is refreshed per request.
        for script in doc.select(&selector("script")?) {
            let text = html::text(script);
            let Some((_, config)) = text.split_once("var acLockRelease = ") else {
                continue;
            };
            let config = config.split(';').next().unwrap_or_default();
            if let Ok(value) = serde_json::from_str::<Value>(config.trim()) {
                if let Some(nonce) = value["nonce"].as_str().filter(|s| {
                    !s.is_empty() && s.len() <= 128 && s.bytes().all(|c| c.is_ascii_alphanumeric())
                }) {
                    return Ok((id, nonce.into()));
                }
            }
        }
        Err(Error::new(
            "Rainy Lotus chapter request configuration is missing",
        ))
    }

    fn chapter_response(
        value: &Value,
        root: &str,
        page: u32,
        pages: u32,
    ) -> Result<Vec<NovelChapter>> {
        let data = &value["data"];
        if value["success"] != true
            || data["page"].as_u64() != Some(page as u64)
            || data["max_pages"].as_u64() != Some(pages as u64)
        {
            return Err(Error::new(
                "Rainy Lotus chapter pagination changed or was rejected; refresh and try again",
            ));
        }
        let fragment = data["html"]
            .as_str()
            .ok_or_else(|| Error::new("Rainy Lotus chapter response is missing"))?;
        let entries = Self::chapter_rows(
            &html::document(&format!("<div id=chapters>{fragment}</div>")),
            root,
        )?;
        if entries.is_empty() {
            return Err(Error::new("Rainy Lotus chapter page is empty"));
        }
        Ok(entries)
    }

    fn load_chapter_page(
        &self,
        doc: &Html,
        root: &str,
        page: u32,
        pages: u32,
    ) -> Result<Vec<NovelChapter>> {
        if page == 1 {
            return Self::chapter_rows(doc, root);
        }
        let (id, nonce) = Self::chapter_request_metadata(doc)?;
        let response = self
            .client
            .post(format!("{BASE}/wp-admin/admin-ajax.php"))
            .form(&[
                ("action", "ac_novel_chapter_page"),
                ("nonce", nonce.as_str()),
                ("novel_id", id.as_str()),
                ("chapter_page", &page.to_string()),
                ("order", "DESC"),
                ("search", ""),
            ])
            .send()?
            .error_for_status()?;
        site_url(response.final_url())?;
        let value = serde_json::from_str(response.text()?)
            .map_err(|_| Error::new("Rainy Lotus chapter response is not JSON"))?;
        Self::chapter_response(&value, root, page, pages)
    }

    fn reader(doc: &Html, url: &str, title: Option<String>) -> Result<NovelText> {
        // Recheck the server page: a formerly locked chapter may now be released
        // or owned. A hidden generic unlock modal is present on free pages too.
        if doc
            .select(&selector("body.ac-chapter-gate, .post-password-form")?)
            .next()
            .is_some()
        {
            return Err(Error::new("This Rainy Lotus chapter is locked. Open it on the website to sign in or unlock it, then refresh the chapter list. No coins are spent automatically."));
        }
        chapter_text(doc, "#ac-r-body", url, title)
    }

    fn filter_rows(doc: &Html) -> Result<Vec<FilterDefinition>> {
        let mut filters = Vec::new();
        for (id, name) in [
            ("genre", "Genre"),
            ("tag", "Tag"),
            ("status", "Status"),
            ("sort", "Sort order"),
        ] {
            let select = doc
                .select(&selector(&format!("#ac-archive-filter-{id}"))?)
                .next()
                .ok_or_else(|| Error::new("Rainy Lotus filter metadata is missing"))?;
            let mut options: Vec<_> = select
                .select(&selector("option")?)
                .map(|n| OptionItem {
                    label: normalize_space(&html::text(n)),
                    value: n.value().attr("value").unwrap_or_default().into(),
                })
                .collect();
            if id == "sort" {
                if let Some(option) = options.iter_mut().find(|option| option.value.is_empty()) {
                    option.label = "Use listing order".into();
                }
            }
            if options.is_empty() {
                return Err(Error::new("Rainy Lotus filter options are empty"));
            }
            filters.push(FilterDefinition::Select {
                id: id.into(),
                name: name.into(),
                options,
                default_index: 0,
            });
        }
        Ok(filters)
    }
}

impl NovelSource for RainyLotus {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse("", page, true, &json!({}))
    }
    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse("", page, false, &json!({}))
    }
    fn listing(&mut self, id: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        match id {
            "popular" => self.browse("", page, true, filters),
            "latest" => self.browse("", page, false, filters),
            _ => Err(Error::new("Unsupported Rainy Lotus listing")),
        }
    }
    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        self.browse(query, page, false, filters)
    }
    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let url = novel_url(item.url.as_deref().unwrap_or(&item.key))?;
        Self::detail(&self.document(&url)?, &url)
    }
    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        let url = novel_url(item.url.as_deref().unwrap_or(&item.key))?;
        let doc = self.document(&url)?;
        let pages = Self::chapter_page_count(&doc)?;
        let mut entries = Vec::new();
        let mut seen = BTreeSet::new();
        for page in 1..=pages {
            for entry in self.load_chapter_page(&doc, &url, page, pages)? {
                if !seen.insert(entry.key.clone()) {
                    return Err(Error::new(
                        "Rainy Lotus chapter pagination repeated a chapter; refresh and try again",
                    ));
                }
                entries.push(entry);
            }
        }
        Ok(entries)
    }
    fn chapters_page(&mut self, item: CatalogItem, page: u32) -> Result<NovelChapterPage> {
        let url = novel_url(item.url.as_deref().unwrap_or(&item.key))?;
        let doc = self.document(&url)?;
        let pages = Self::chapter_page_count(&doc)?;
        let page = page.max(1);
        if page > pages {
            return Err(Error::new("Rainy Lotus chapter page is out of range"));
        }
        Ok(NovelChapterPage {
            entries: self.load_chapter_page(&doc, &url, page, pages)?,
            has_next_page: page < pages,
            page_count: Some(pages),
        })
    }
    fn text(&mut self, item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let url = chapter.url.as_deref().unwrap_or(&chapter.key);
        if novel_url(url)? != novel_url(item.url.as_deref().unwrap_or(&item.key))? {
            return Err(Error::new(
                "Rainy Lotus chapter belongs to a different novel",
            ));
        }
        Self::reader(&self.document(url)?, url, chapter.title)
    }
    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Self::filter_rows(&self.document(&format!("{BASE}/series/"))?)
    }
    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let Ok(url) = novel_url(candidate) else {
            return Ok(None);
        };
        let mut item = CatalogItem::new(&url, "");
        item.url = Some(url);
        item.language = Some("en".into());
        Ok(Some(UrlResolveResult {
            item: Some(item),
            ..Default::default()
        }))
    }
    fn authenticate(&mut self, request: AuthenticationRequest) -> Result<AuthenticationState> {
        if request.interactive {
            Ok(login_action())
        } else {
            self.authentication_status()
        }
    }
    fn authentication_status(&mut self) -> Result<AuthenticationState> {
        let doc = self.document(BASE)?;
        if doc
            .select(&selector("body.ac-logged-out")?)
            .next()
            .is_some()
        {
            return Ok(login_action());
        }
        if doc
            .select(&selector("body.logged-in, body.ac-logged-in")?)
            .next()
            .is_none()
        {
            return Err(Error::new("Rainy Lotus login status markup is missing"));
        }
        Ok(AuthenticationState {
            authenticated: true,
            message: Some("Signed in. Unlock paid chapters on the official website.".into()),
            ..Default::default()
        })
    }
    fn logout(&mut self) -> Result<AuthenticationState> {
        let mut snapshot = manatan_sdk::cookies::get(BASE).map_err(Error::new)?;
        for cookie in &mut snapshot.cookies {
            cookie.value.clear();
            cookie.expires_at = Some(0);
        }
        manatan_sdk::cookies::set(BASE, &snapshot.cookies).map_err(Error::new)?;
        let _: browser::WebViewResponse = browser::open(&WebViewRequest {
            url: BASE.into(),
            session: Some(WebViewSession {
                id: PROFILE.into(),
                clear: true,
                ..Default::default()
            }),
            ..Default::default()
        })?;
        Ok(login_action())
    }
}

#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel("rainylotus", RainyLotus::default())
}
#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;
    const NOVEL: &str = "https://rainylotus.com/novel/example";
    #[test]
    fn server_pagination_metadata_is_validated_without_executing_scripts() {
        let doc = html::document(
            r#"<body class="single postid-42"><span id="ac-chapter-page-label">1 / 2</span><script>var acLockRelease = {"nonce":"fixture123","ajaxUrl":"https://untrusted.invalid"};</script></body>"#,
        );
        assert_eq!(RainyLotus::chapter_page_count(&doc).unwrap(), 2);
        assert_eq!(
            RainyLotus::chapter_request_metadata(&doc).unwrap(),
            ("42".into(), "fixture123".into())
        );
        assert_eq!(
            RainyLotus::chapter_page_count(&html::document("<div id=chapters></div>")).unwrap(),
            1
        );
        assert!(RainyLotus::chapter_request_metadata(&html::document(
            "<script>arbitrary()</script>"
        ))
        .is_err());
        assert!(RainyLotus::chapter_page_count(&html::document(
            "<span id=ac-chapter-page-label>1 / 99999</span>"
        ))
        .is_err());
    }
    #[test]
    fn older_server_page_preserves_public_access_and_rejects_repeated_page_numbers() {
        let value = json!({"success":true,"data":{"page":2,"max_pages":2,"html":"<div class=chapter-row data-num=1><a class=chapter-el href='/novel/example/chapter-1' data-ac-locked=0 data-ac-chapter-label='Chapter 1'>Chapter 1</a></div>"}});
        let entries = RainyLotus::chapter_response(&value, NOVEL, 2, 2).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].chapter_number, Some(1.0));
        assert!(!entries[0].is_locked);
        assert!(RainyLotus::chapter_response(&value, NOVEL, 3, 2).is_err());
        assert!(RainyLotus::chapter_response(&value, NOVEL, 2, 3).is_err());
        assert!(RainyLotus::chapter_response(&json!({"success":false}), NOVEL, 2, 2).is_err());
        assert!(RainyLotus::chapter_response(
            &json!({"success":true,"data":{"page":2,"max_pages":2,"html":""}}),
            NOVEL,
            2,
            2
        )
        .is_err());
    }
    #[test]
    fn filter_definitions_preserve_live_values_and_empty_defaults() {
        let mut fixture = String::new();
        for id in ["genre", "tag", "status", "sort"] {
            fixture.push_str(&format!("<select id='ac-archive-filter-{id}'><option value=''>Any</option><option value='fantasy'>Fantasy</option></select>"));
        }
        let filters = RainyLotus::filter_rows(&html::document(&fixture)).unwrap();
        assert_eq!(filters.len(), 4);
        let FilterDefinition::Select {
            id,
            options,
            default_index,
            ..
        } = &filters[0]
        else {
            panic!("Expected select");
        };
        assert_eq!(id, "genre");
        assert_eq!(options[0].value, "");
        assert_eq!(options[1].value, "fantasy");
        assert_eq!(*default_index, 0);
        assert!(RainyLotus::filter_rows(&html::document("No metadata")).is_err());
    }
    #[test]
    fn login_action_uses_official_origin_and_source_profile() {
        let state = login_action();
        let Some(AuthenticationAction::WebView {
            url,
            profile,
            cookie_url,
            completion,
        }) = state.action
        else {
            panic!("Expected official login");
        };
        assert_eq!(url, format!("{BASE}/login"));
        assert_eq!(profile, PROFILE);
        assert_eq!(cookie_url.as_deref(), Some(BASE));
        assert!(completion.allow_manual_close);
        assert!(!state.authenticated);
    }
    #[test]
    fn search_pagination_and_filters_follow_public_archive_contract() {
        let url = RainyLotus::catalog_url(
            "A & B",
            2,
            false,
            &json!({"genre":"fantasy", "status":"ongoing", "sort":"title"}),
        )
        .unwrap();
        assert!(url.contains("/series/page/2/?q=A+%26+B&sort=title&genre=fantasy&status=ongoing"));
        assert!(RainyLotus::catalog_url("", 1, true, &json!({}))
            .unwrap()
            .ends_with("?sort=views"));
        assert!(RainyLotus::catalog_url("", 1, true, &json!({"sort":""}))
            .unwrap()
            .ends_with("?sort=views"));
        assert!(RainyLotus::catalog_url("", 1, false, &json!({"sort":"updated"})).is_err());
        assert!(
            RainyLotus::catalog_url("", 1, false, &json!({"genre":"x&redirect=evil"})).is_err()
        );
    }
    #[test]
    fn catalog_empty_search_is_distinct_from_missing_markup() {
        let doc = html::document(
            r#"<div id="ac-archive-results"><article class="ac-grid2-card"><a class="ac-grid2-title-link" href="/novel/example"><h3>Example</h3></a><img class="ac-grid2-cover-img" src="/cover.webp"><div class="ac-grid2-genres"><a>Yaoi,</a></div></article></div><div id="ac-archive-pagination"><a class="next page-numbers" href="/series/page/2/">Next</a></div>"#,
        );
        let list = RainyLotus::catalog(&doc).unwrap();
        assert!(list.has_next_page);
        assert_eq!(list.entries[0].key, NOVEL);
        assert_eq!(list.entries[0].content_rating.as_deref(), Some("general"));
        assert_eq!(
            list.entries[0].cover.as_ref().unwrap().url,
            format!("{BASE}/cover.webp")
        );
        assert!(RainyLotus::catalog(&html::document(
            "<div id=ac-archive-results>No results</div>"
        ))
        .unwrap()
        .entries
        .is_empty());
        assert!(RainyLotus::catalog(&html::document("Upstream failed")).is_err());
    }
    #[test]
    fn complete_index_preserves_parts_order_and_access_flags() {
        let mut fixture = String::from("<div id=chapters>");
        for n in (1..=150).rev() {
            fixture.push_str(&format!("<div class=chapter-row data-num={n}><a class=chapter-el href='/novel/example/chapter-{n}' data-ac-chapter-label='Chapter {n}' data-ac-locked={}></a></div>", if n > 10 { 1 } else { 0 }));
        }
        fixture.push_str("<div class=chapter-row data-num=1><a class=chapter-el href='/novel/example/chapter-1-part-2' data-ac-chapter-label='Chapter 1 Part 2' data-ac-locked=0></a></div></div>");
        let chapters = RainyLotus::chapter_rows(&html::document(&fixture), NOVEL).unwrap();
        assert_eq!(chapters.len(), 151);
        assert_eq!(chapters[0].chapter_number, Some(150.));
        assert!(chapters[0].is_locked);
        assert!(!chapters[149].is_locked);
        assert_eq!(chapters[149].chapter_number, chapters[150].chapter_number);
        assert_ne!(chapters[149].key, chapters[150].key);
    }
    #[test]
    fn missing_paid_access_metadata_and_cross_novel_links_are_errors() {
        for link in [
            "<a class=chapter-el href='/novel/example/chapter-1' data-ac-cost=10>",
            "<a class=chapter-el href='/novel/other/chapter-1' data-ac-locked=0>",
        ] {
            assert!(RainyLotus::chapter_rows(
                &html::document(&format!(
                    "<div id=chapters><div class=chapter-row>{link}Chapter 1</a></div></div>"
                )),
                NOVEL
            )
            .is_err());
        }
        assert!(RainyLotus::chapter_rows(&html::document("Unavailable"), NOVEL).is_err());
    }
    #[test]
    fn public_rows_without_unlock_attributes_use_the_server_reading_label() {
        let doc = html::document("<div id=chapters><div class=chapter-row data-num=1 data-ac-reading-label='Chapter 1'><a class=chapter-el href='/novel/example/chapter-1'><span>Chapter 1</span><span>6d ago</span></a></div></div>");
        let entries = RainyLotus::chapter_rows(&doc, NOVEL).unwrap();
        assert!(!entries[0].is_locked);
        assert_eq!(entries[0].title.as_deref(), Some("Chapter 1"));
    }
    #[test]
    fn hidden_unlock_dialog_does_not_block_free_reader_or_leak_navigation() {
        let doc = html::document("<body><header>Navigation</header><div id=ac-r-body><h1>Chapter 1</h1><p>Public <em>story</em>.</p><script>tracking()</script></div><div id=acl-msg>This chapter is locked.</div><footer>Recommendations</footer></body>");
        let text = RainyLotus::reader(&doc, &format!("{NOVEL}/chapter-1"), None)
            .unwrap()
            .html
            .unwrap();
        assert!(text.contains("Public <em>story</em>"));
        for excluded in [
            "Navigation",
            "locked",
            "Recommendations",
            "tracking",
            "script",
        ] {
            assert!(!text.contains(excluded));
        }
    }
    #[test]
    fn server_gate_wins_even_if_reader_markup_exists() {
        let doc = html::document(
            "<body class=ac-chapter-gate><div id=ac-r-body><p>Preview only</p></div></body>",
        );
        assert!(RainyLotus::reader(&doc, NOVEL, None)
            .unwrap_err()
            .to_string()
            .contains("locked"));
        assert!(RainyLotus::reader(&html::document("No content"), NOVEL, None).is_err());
    }
    #[test]
    fn metadata_uses_reading_language_and_explicit_adult_tags_only() {
        let doc = html::document("<h1 id=ac-novel-title>Example</h1><div id=ac-cover-img><img src='/cover.webp'></div><div id=synopsis-content><p>Synopsis</p></div><div id=ac-genres-row><a>Yaoi</a><a>Fantasy</a></div><div id=ac-meta-status><a class=ac-meta-chip>Completed</a></div><div id=ac-meta-origin><a class=ac-meta-chip>Chinese</a></div>");
        let detail = RainyLotus::detail(&doc, NOVEL).unwrap();
        assert_eq!(detail.language.as_deref(), Some("en"));
        assert_eq!(detail.extra["originalLanguage"], "Chinese");
        assert_eq!(detail.status, Some(json!("completed")));
        assert_eq!(detail.content_rating.as_deref(), Some("general"));
        assert_eq!(content_rating(&["R-18".into()]), "adult");
        assert_eq!(content_rating(&["Smut".into()]), "adult");
    }
    #[test]
    fn chapter_url_resolves_to_canonical_novel_and_rejects_foreign_origins() {
        assert_eq!(
            novel_url(&format!("{NOVEL}/chapter-1?tracking=1#note")).unwrap(),
            NOVEL
        );
        assert!(novel_url("https://rainylotus.com.evil.example/novel/example").is_err());
        assert!(novel_url("http://rainylotus.com/novel/example").is_err());
        assert!(novel_url("https://rainylotus.com/series").is_err());
        assert!(novel_url("https://rainylotus.com:444/novel/example").is_err());
    }
}
