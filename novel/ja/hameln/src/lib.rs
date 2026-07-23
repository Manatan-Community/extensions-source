use chrono::DateTime;
use manatan_common::{absolute_url, attr, normalize_space, require, selector};
use manatan_sdk::{
    browser::{
        self, WebViewRequest, WebViewResponse, WebViewScript, WebViewSession, WebViewWait,
        WebViewWaitUntil,
    },
    html::{self, ElementRef, Html},
    model::{
        CatalogItem, FilterDefinition, ImageRequest, ImageRequestContext, NovelChapter,
        NovelChapterPage, NovelContentBlock, NovelText, OptionItem, Paged, UrlResolveResult,
    },
    Error, NovelSource, Result,
};
use serde_json::{json, Value};
use url::Url;

#[cfg(target_arch = "wasm32")]
const SOURCE_ID: &str = "hameln";
const BASE_URL: &str = "https://syosetu.org";
const IMAGE_URL: &str = "https://img.syosetu.org";
const CHALLENGE_TIMEOUT_MS: u64 = 45_000;
const MAX_RANKING_RESULTS: usize = 100;
const TEST_USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) \
AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1";

pub struct HamelnSource;

impl Default for HamelnSource {
    fn default() -> Self {
        Self
    }
}

impl HamelnSource {
    fn document(&self, url: &str) -> Result<(Html, String, String)> {
        // Hameln currently challenges non-browser HTTP clients on every page.
        // Load through the host-owned browser so each platform uses its real
        // browser user agent and keeps clearance state in this source-local
        // profile. No page script is exposed to the extension.
        let rendered_html_script = format!(
            r#"(() => {{
                const clone = document.documentElement.cloneNode(true);
                clone.querySelectorAll('script, style, noscript, iframe, link, svg').forEach(node => node.remove());
                const cards = clone.querySelectorAll("div.section3[id^='nid_'], div.search_box[id^='nid_']");
                cards.forEach((card, index) => {{ if (index >= {MAX_RANKING_RESULTS}) card.remove(); }});
                return clone.outerHTML;
            }})()"#,
        );
        let response: WebViewResponse = browser::open(&WebViewRequest {
            url: url.to_owned(),
            cookie_url: Some(BASE_URL.to_owned()),
            session: Some(WebViewSession {
                id: "hameln-cloudflare".to_owned(),
                ..WebViewSession::default()
            }),
            wait_for: Some(WebViewWait::Script {
                script: r#"document.readyState === "complete" &&
                    document.title !== "Just a moment..." &&
                    !document.getElementById("challenge-error-title") &&
                    !document.getElementById("challenge-error-text") &&
                    !document.querySelector('.cf-turnstile, [name="cf-turnstile-response"]')"#
                    .to_owned(),
            }),
            wait_until: Some(WebViewWaitUntil::LoadFinished),
            headers: vec![
                ("Referer".to_owned(), BASE_URL.to_owned()),
                (
                    "Accept".to_owned(),
                    "text/html,application/xhtml+xml".to_owned(),
                ),
            ],
            timeout_ms: Some(CHALLENGE_TIMEOUT_MS),
            return_html: false,
            scripts: vec![
                WebViewScript {
                    id: Some("hameln-user-agent".to_owned()),
                    script: "navigator.userAgent".to_owned(),
                    run_at: None,
                },
                WebViewScript {
                    id: Some("hameln-html".to_owned()),
                    script: rendered_html_script,
                    run_at: None,
                },
            ],
            ..WebViewRequest::default()
        })?;
        let user_agent = response
            .script_results
            .iter()
            .find(|result| result.id.as_deref() == Some("hameln-user-agent"))
            .and_then(|result| result.value.as_ref())
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| Error::new("Hameln browser returned no user agent"))?
            .to_owned();
        let rendered = response
            .script_results
            .iter()
            .find(|result| result.id.as_deref() == Some("hameln-html"))
            .and_then(|result| result.value.as_ref())
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| Error::new("Hameln browser returned no rendered HTML"))?;
        Ok((html::document(&rendered), response.final_url, user_agent))
    }

    fn prepare_image_host(&self, page: &Paged<CatalogItem>) -> Result<()> {
        let Some(url) = page
            .entries
            .iter()
            .filter_map(|item| item.cover.as_ref())
            .map(|cover| cover.url.as_str())
            .find(|url| url.starts_with(IMAGE_URL))
        else {
            return Ok(());
        };

        // Cloudflare challenges img.syosetu.org independently from the main
        // site. Let the host-owned browser establish that origin's clearance
        // once; the artwork proxy then reuses the source-scoped cookie jar.
        let _: WebViewResponse = browser::open(&WebViewRequest {
            url: url.to_owned(),
            cookie_url: Some(IMAGE_URL.to_owned()),
            session: Some(WebViewSession {
                id: "hameln-cloudflare".to_owned(),
                ..WebViewSession::default()
            }),
            wait_for: Some(WebViewWait::Script {
                script: r#"document.readyState === "complete" &&
                    document.title !== "Just a moment..." &&
                    !document.getElementById("challenge-error-title") &&
                    Array.from(document.images).some(image => image.complete && image.naturalWidth > 0)"#
                    .to_owned(),
            }),
            wait_until: Some(WebViewWaitUntil::LoadFinished),
            headers: vec![("Referer".to_owned(), BASE_URL.to_owned())],
            timeout_ms: Some(CHALLENGE_TIMEOUT_MS),
            ..WebViewRequest::default()
        })?;
        Ok(())
    }

    fn ranking_url(filters: &Value) -> String {
        let mode = filter(filters, "period", "rank");
        format!("{BASE_URL}/?mode={mode}")
    }

    fn search_url(query: &str, page: u32) -> Result<String> {
        let mut url = Url::parse(&format!("{BASE_URL}/search/"))
            .map_err(|error| Error::new(error.to_string()))?;
        url.query_pairs_mut()
            .append_pair("mode", "search")
            .append_pair("word", query);
        if page > 1 {
            url.query_pairs_mut().append_pair("page", &page.to_string());
        }
        Ok(url.to_string())
    }

    fn parse_cards(document: &Html, page: u32, user_agent: &str) -> Result<Paged<CatalogItem>> {
        let rows = selector("div.section3[id^='nid_'], div.search_box[id^='nid_']")?;
        let title = selector(
            ".blo_title_base > a[href*='/novel/'], a.search_novel_title[href*='/novel/']",
        )?;
        let author = selector(".blo_title_sak, .trigger > p:first-of-type")?;
        let summary = selector(".blo_inword, .toggle_container > p:first-child")?;
        let status = selector(".blo_wasuu_base span[title], .trigger > p:last-child")?;
        let tag_links = selector(".all_keyword a[href*='/search/']")?;
        let mut entries = Vec::new();

        for row in document.select(&rows) {
            let Some(anchor) = row.select(&title).next() else {
                continue;
            };
            let Some(href) = attr(anchor, "href") else {
                continue;
            };
            let Some(work_url) = canonical_work_url(&href) else {
                continue;
            };
            let name = strip_ranking_prefix(&normalize_space(&html::text(anchor)));
            if name.is_empty() {
                continue;
            }

            let mut tags = row
                .select(&tag_links)
                .map(html::text)
                .map(|value| normalize_space(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            if tags.is_empty() {
                tags = mobile_card_tags(row)?;
            }
            if tags.iter().any(|tag| is_adult_tag(tag)) {
                continue;
            }

            let mut item = CatalogItem::new(work_url.clone(), name);
            item.url = Some(work_url.clone());
            item.cover = cover_url(&work_url).map(|url| image_request(&url, user_agent));
            item.description = row
                .select(&summary)
                .next()
                .map(html::text)
                .map(|value| normalize_space(&value))
                .filter(|value| !value.is_empty());
            item.authors = row
                .select(&author)
                .next()
                .and_then(parse_author)
                .into_iter()
                .collect();
            item.tags = tags;
            item.status = row
                .select(&status)
                .next()
                .map(|element| {
                    let value = attr(element, "title").unwrap_or_else(|| html::text(element));
                    status_value(&value)
                })
                .map(|value| json!(value));
            item.language = Some("ja".into());
            item.content_rating = Some(card_content_rating(&item.tags).into());
            entries.push(item);
        }

        Ok(Paged::new(entries, has_next_page(document, page)?))
    }

    fn parse_details(document: &Html, work_url: &str, user_agent: &str) -> Result<CatalogItem> {
        let title = require(
            first_text(document, "[itemprop='name']")?.or_else(|| {
                meta_content(document, "meta[property='og:title']")
                    .ok()
                    .flatten()
            }),
            "Hameln work has no title",
        )?;
        let author = first_text(document, "[itemprop='author']")?
            .map(|value| value.trim_start_matches("作：").trim().to_owned())
            .filter(|value| !value.is_empty());
        let sections = selector("#maind > .ss")?;
        let description = document
            .select(&sections)
            .nth(1)
            .map(html::text)
            .map(|value| normalize_space(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| {
                meta_content(document, "meta[property='og:description']")
                    .ok()
                    .flatten()
            });
        let keywords = selector("[itemprop='keywords'] a")?;
        let genre = first_text(document, "[itemprop='genre']")?;
        let mut tags = document
            .select(&keywords)
            .map(html::text)
            .map(|value| normalize_space(&value))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        if let Some(genre) = genre.filter(|value| !tags.contains(value)) {
            tags.push(genre);
        }
        if tags.iter().any(|tag| is_adult_tag(tag)) {
            return Err(Error::new("Hameln adult works are not supported"));
        }

        let mut item = CatalogItem::new(work_url, title);
        item.url = Some(work_url.to_owned());
        item.cover = meta_content(document, "meta[property='og:image']")?
            .or_else(|| cover_url(work_url))
            .map(|url| image_request(&url, user_agent));
        item.description = description;
        item.authors = author.into_iter().collect();
        item.tags = tags;
        item.status = Some(json!("unknown"));
        item.initialized = true;
        item.language = Some("ja".into());
        item.content_rating = Some(card_content_rating(&item.tags).into());
        Ok(item)
    }

    fn parse_chapters(document: &Html, work_url: &str) -> Result<Vec<NovelChapter>> {
        let rows = selector("#maind table tr, ul.entry > li")?;
        let link = selector("a[href$='.html']")?;
        let time = selector("time[datetime]")?;
        let mut chapters = Vec::new();
        for row in document.select(&rows) {
            let Some(anchor) = row.select(&link).next() else {
                continue;
            };
            let Some(href) = attr(anchor, "href") else {
                continue;
            };
            let title = chapter_anchor_title(anchor);
            if title.is_empty() {
                continue;
            }
            let url = absolute_url(work_url, &href)?;
            let date_uploaded = row
                .select(&time)
                .next()
                .and_then(|element| attr(element, "datetime"))
                .and_then(|value| parse_datetime(&value));
            chapters.push(NovelChapter {
                key: url.clone(),
                title: Some(title),
                chapter_number: chapter_number_from_url(&url),
                date_uploaded,
                url: Some(url),
                language: Some("ja".into()),
                source_order: Some(chapters.len() as i32),
                page: Some(1),
                ..NovelChapter::default()
            });
        }
        Ok(chapters)
    }

    fn parse_text(
        document: &Html,
        page_url: &str,
        title: Option<String>,
        user_agent: &str,
    ) -> Result<NovelText> {
        let body = html_for(document, "#honbun")?
            .ok_or_else(|| Error::new("Hameln chapter has no readable body"))?;
        let preface = html_for(document, "#maegaki")?;
        let afterword = html_for(document, "#atogaki")?;
        let mut rendered = String::new();
        if let Some(title) = title.as_ref().filter(|value| !value.is_empty()) {
            rendered.push_str("<h1>");
            rendered.push_str(title);
            rendered.push_str("</h1>");
        }
        if let Some(preface) = preface.filter(|value| !value.trim().is_empty()) {
            rendered.push_str("<aside>");
            rendered.push_str(&preface);
            rendered.push_str("</aside>");
        }
        rendered.push_str(&body);
        if let Some(afterword) = afterword.filter(|value| !value.trim().is_empty()) {
            rendered.push_str("<aside>");
            rendered.push_str(&afterword);
            rendered.push_str("</aside>");
        }
        Ok(NovelText {
            html: Some(rendered.clone()),
            title,
            base_url: Some(page_url.to_owned()),
            image_context: Some(ImageRequestContext {
                headers: [
                    ("Referer".to_owned(), page_url.to_owned()),
                    ("User-Agent".to_owned(), user_agent.to_owned()),
                ]
                .into_iter()
                .collect(),
                cookie_url: Some(BASE_URL.to_owned()),
            }),
            blocks: vec![NovelContentBlock::Text {
                text: rendered,
                html: true,
            }],
            ..NovelText::default()
        })
    }

    fn work_url(item: &CatalogItem) -> Result<String> {
        canonical_work_url(item.url.as_deref().unwrap_or(&item.key))
            .ok_or_else(|| Error::new("Hameln URL has no novel id"))
    }
}

impl NovelSource for HamelnSource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.listing("popular", page, &json!({}))
    }

    fn listing(&mut self, listing: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        if listing != "popular" {
            return Err(Error::new(format!("unknown novel listing {listing:?}")));
        }
        if page > 1 {
            return Ok(Paged::new(Vec::new(), false));
        }
        let (document, _, user_agent) = self.document(&Self::ranking_url(filters))?;
        let mut result = Self::parse_cards(&document, 1, &user_agent)?;
        result.has_next_page = false;
        self.prepare_image_host(&result)?;
        Ok(result)
    }

    fn search(&mut self, query: &str, page: u32, _filters: &Value) -> Result<Paged<CatalogItem>> {
        let page = page.max(1);
        let (document, _, user_agent) = self.document(&Self::search_url(query, page)?)?;
        let result = Self::parse_cards(&document, page, &user_agent)?;
        self.prepare_image_host(&result)?;
        Ok(result)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let work_url = Self::work_url(&item)?;
        let (document, final_url, user_agent) = self.document(&work_url)?;
        let canonical = canonical_work_url(&final_url).unwrap_or(work_url);
        Self::parse_details(&document, &canonical, &user_agent)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        let work_url = Self::work_url(&item)?;
        let (document, final_url, _) = self.document(&work_url)?;
        let canonical = canonical_work_url(&final_url).unwrap_or(work_url);
        Self::parse_chapters(&document, &canonical)
    }

    fn chapters_page(&mut self, item: CatalogItem, page: u32) -> Result<NovelChapterPage> {
        if page > 1 {
            return Ok(NovelChapterPage {
                entries: Vec::new(),
                has_next_page: false,
                page_count: Some(1),
            });
        }
        Ok(NovelChapterPage {
            entries: self.chapters(item)?,
            has_next_page: false,
            page_count: Some(1),
        })
    }

    fn text(&mut self, _item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let url = absolute_url(BASE_URL, chapter.url.as_deref().unwrap_or(&chapter.key))?;
        let (document, final_url, user_agent) = self.document(&url)?;
        Self::parse_text(&document, &final_url, chapter.title, &user_agent)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![FilterDefinition::Select {
            id: "period".into(),
            name: "Ranking period".into(),
            options: RANKING_PERIODS
                .iter()
                .map(|(label, value)| OptionItem {
                    label: (*label).into(),
                    value: (*value).into(),
                })
                .collect(),
            default_index: 0,
        }])
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let url = Url::parse(candidate).map_err(|error| Error::new(error.to_string()))?;
        if url.host_str() != Some("syosetu.org") {
            return Ok(None);
        }
        let parts = url
            .path_segments()
            .map(|parts| parts.filter(|part| !part.is_empty()).collect::<Vec<_>>())
            .unwrap_or_default();
        if parts.first().copied() != Some("novel") || parts.len() < 2 {
            return Ok(None);
        }
        let work_url = format!("{BASE_URL}/novel/{}/", parts[1]);
        let mut item = CatalogItem::new(work_url.clone(), "");
        item.url = Some(work_url);
        item.cover = cover_url(candidate).map(|url| image_request(&url, TEST_USER_AGENT));
        item.language = Some("ja".into());
        item.content_rating = Some("suggestive".into());
        let novel_chapter = parts.get(2).and_then(|part| {
            part.strip_suffix(".html")?
                .parse::<f32>()
                .ok()
                .map(|number| NovelChapter {
                    key: candidate.to_owned(),
                    url: Some(candidate.to_owned()),
                    chapter_number: Some(number),
                    language: Some("ja".into()),
                    ..NovelChapter::default()
                })
        });
        Ok(Some(UrlResolveResult {
            item: Some(item),
            novel_chapter,
            ..UrlResolveResult::default()
        }))
    }
}

fn canonical_work_url(candidate: &str) -> Option<String> {
    let absolute = absolute_url(BASE_URL, candidate).ok()?;
    let url = Url::parse(&absolute).ok()?;
    if url.host_str() != Some("syosetu.org") {
        return None;
    }
    let parts = url
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.first().copied() != Some("novel") || parts.len() < 2 {
        return None;
    }
    let id = parts[1];
    if id.is_empty() || !id.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    Some(format!("{BASE_URL}/novel/{id}/"))
}

fn cover_url(candidate: &str) -> Option<String> {
    let work = canonical_work_url(candidate)?;
    let id = Url::parse(&work).ok()?.path_segments()?.nth(1)?.to_owned();
    Some(format!("{IMAGE_URL}/ogp_{id}"))
}

fn image_request(url: &str, user_agent: &str) -> ImageRequest {
    ImageRequest::get(url)
        .header("Referer", BASE_URL)
        .header("User-Agent", user_agent)
        // Cookie scopes are origin-bound by the host. Hameln serves covers
        // from img.syosetu.org rather than the page origin.
        .cookies_for(IMAGE_URL)
}

fn parse_author(element: ElementRef<'_>) -> Option<String> {
    let value = normalize_space(&html::text(element));
    let author = value
        .strip_prefix("作者：")
        .or_else(|| value.rsplit_once("作：").map(|(_, author)| author));
    author
        .map(|author| author.trim().to_owned())
        .filter(|author| !author.is_empty())
}

fn strip_ranking_prefix(value: &str) -> String {
    let Some((prefix, title)) = value.split_once('位') else {
        return value.to_owned();
    };
    if prefix
        .trim()
        .chars()
        .all(|character| character.is_ascii_digit())
    {
        title.trim().to_owned()
    } else {
        value.to_owned()
    }
}

fn mobile_card_tags(row: ElementRef<'_>) -> Result<Vec<String>> {
    let paragraphs = selector(".trigger > p")?;
    Ok(row
        .select(&paragraphs)
        .map(html::text)
        .map(|value| normalize_space(&value))
        .find_map(|value| value.strip_prefix("タグ：").map(str::to_owned))
        .map(|value| {
            value
                .split_whitespace()
                .map(str::to_owned)
                .filter(|tag| !tag.is_empty())
                .collect()
        })
        .unwrap_or_default())
}

fn status_value(value: &str) -> &'static str {
    if value.contains("完結") || value.contains("短編") {
        "completed"
    } else if value.contains("連載") {
        "ongoing"
    } else {
        "unknown"
    }
}

fn card_content_rating(tags: &[String]) -> &'static str {
    if tags.iter().any(|tag| {
        let normalized = tag.to_ascii_lowercase();
        normalized.contains("r-15")
            || tag.contains("残酷")
            || tag.contains("暴力")
            || tag.contains("性的")
    }) {
        "suggestive"
    } else {
        "safe"
    }
}

fn is_adult_tag(tag: &str) -> bool {
    let normalized = tag.to_ascii_lowercase();
    normalized.contains("r-18")
        || normalized.contains("r18")
        || normalized.contains("adult")
        || tag.contains("18禁")
}

fn has_next_page(document: &Html, current: u32) -> Result<bool> {
    let links = selector("a[href*='page=']")?;
    Ok(document.select(&links).any(|element| {
        attr(element, "href")
            .and_then(|href| Url::parse(&absolute_url(BASE_URL, &href).ok()?).ok())
            .and_then(|url| {
                url.query_pairs()
                    .find(|(key, _)| key == "page")
                    .and_then(|(_, value)| value.parse::<u32>().ok())
            })
            .is_some_and(|page| page > current)
    }))
}

fn first_text(document: &Html, query: &str) -> Result<Option<String>> {
    let selector = selector(query)?;
    Ok(document
        .select(&selector)
        .next()
        .map(html::text)
        .map(|value| normalize_space(&value))
        .filter(|value| !value.is_empty()))
}

fn html_for(document: &Html, query: &str) -> Result<Option<String>> {
    let selector = selector(query)?;
    Ok(document
        .select(&selector)
        .next()
        .map(|element| element.inner_html())
        .filter(|value| !value.trim().is_empty()))
}

fn meta_content(document: &Html, query: &str) -> Result<Option<String>> {
    let selector = selector(query)?;
    Ok(document
        .select(&selector)
        .next()
        .and_then(|element| attr(element, "content")))
}

fn parse_datetime(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.timestamp_millis())
}

fn chapter_anchor_title(anchor: ElementRef<'_>) -> String {
    let inner = anchor.inner_html();
    let title_html = inner.split("<br").next().unwrap_or(&inner);
    let fragment = html::fragment(title_html);
    normalize_space(&fragment.root_element().text().collect::<Vec<_>>().join(" "))
}

fn chapter_number_from_url(value: &str) -> Option<f32> {
    Url::parse(value)
        .ok()?
        .path_segments()?
        .find_map(|part| part.strip_suffix(".html")?.parse().ok())
}

fn filter(filters: &Value, key: &str, default: &str) -> String {
    filters
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_owned()
}

const RANKING_PERIODS: &[(&str, &str)] = &[
    ("Daily", "rank"),
    ("Weekly", "rank_week"),
    ("Monthly", "rank_month"),
    ("Quarterly", "rank_3month"),
    ("Yearly", "rank_year"),
    ("All time", "rank_total"),
];

#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel(SOURCE_ID, HamelnSource::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ranking_cards_with_metadata() {
        let document = html::document(include_str!("../tests/fixtures/ranking.html"));
        let page = HamelnSource::parse_cards(&document, 1, TEST_USER_AGENT).unwrap();
        assert_eq!(page.entries.len(), 1);
        let item = &page.entries[0];
        assert_eq!(item.title, "Fixture Story");
        assert_eq!(item.authors, vec!["Fixture Author"]);
        assert_eq!(
            item.cover.as_ref().map(|cover| cover.url.as_str()),
            Some("https://img.syosetu.org/ogp_12345")
        );
        let cover = item.cover.as_ref().unwrap();
        assert_eq!(cover.cookie_url.as_deref(), Some("https://img.syosetu.org"));
        assert_eq!(
            cover.headers.get("Referer").map(String::as_str),
            Some(BASE_URL)
        );
        assert_eq!(item.status, Some(json!("ongoing")));
        assert_eq!(item.content_rating.as_deref(), Some("suggestive"));
        assert!(page.has_next_page);
    }

    #[test]
    fn parses_mobile_ranking_cards() {
        let document = html::document(include_str!("../tests/fixtures/ranking_mobile.html"));
        let page = HamelnSource::parse_cards(&document, 1, TEST_USER_AGENT).unwrap();
        assert_eq!(page.entries.len(), 1);
        let item = &page.entries[0];
        assert_eq!(item.title, "Fixture Mobile Story");
        assert_eq!(item.authors, vec!["Fixture Mobile Author"]);
        assert_eq!(item.status, Some(json!("ongoing")));
        assert_eq!(item.content_rating.as_deref(), Some("suggestive"));
        assert!(item.tags.iter().any(|tag| tag == "R-15"));
        assert_eq!(item.description.as_deref(), Some("Fixture mobile summary."));
    }

    #[test]
    fn parses_details_and_chapters() {
        let document = html::document(include_str!("../tests/fixtures/work.html"));
        let url = "https://syosetu.org/novel/12345/";
        let item = HamelnSource::parse_details(&document, url, TEST_USER_AGENT).unwrap();
        assert_eq!(item.title, "Fixture Story");
        assert_eq!(item.authors, vec!["Fixture Author"]);
        assert!(item
            .description
            .as_deref()
            .unwrap()
            .contains("Fixture summary"));
        let chapters = HamelnSource::parse_chapters(&document, url).unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].title.as_deref(), Some("First chapter"));
        assert_eq!(chapters[1].chapter_number, Some(2.0));
    }

    #[test]
    fn parses_mobile_chapters_without_date_text_in_titles() {
        let document = html::document(include_str!("../tests/fixtures/work_mobile.html"));
        let url = "https://syosetu.org/novel/12345/";
        let chapters = HamelnSource::parse_chapters(&document, url).unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].title.as_deref(), Some("First mobile chapter"));
        assert_eq!(chapters[0].chapter_number, Some(1.0));
        assert_eq!(chapters[1].title.as_deref(), Some("Second mobile chapter"));
        assert_eq!(chapters[1].chapter_number, Some(2.0));
    }

    #[test]
    fn parses_episode_body_and_notes() {
        let document = html::document(include_str!("../tests/fixtures/episode.html"));
        let text = HamelnSource::parse_text(
            &document,
            "https://syosetu.org/novel/12345/1.html",
            Some("First chapter".into()),
            TEST_USER_AGENT,
        )
        .unwrap();
        let rendered = text.html.unwrap();
        assert!(rendered.contains("Preface"));
        assert!(rendered.contains("Fixture body"));
        assert!(rendered.contains("Afterword"));
    }

    #[test]
    fn resolves_work_and_chapter_urls() {
        assert_eq!(
            canonical_work_url("https://syosetu.org/novel/12345/2.html").as_deref(),
            Some("https://syosetu.org/novel/12345/")
        );
        assert_eq!(
            chapter_number_from_url("https://syosetu.org/novel/12345/2.html"),
            Some(2.0)
        );
    }
}
