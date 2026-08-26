use manatan_common::{absolute_url, attr, normalize_space, require, selector};
use manatan_sdk::{
    browser::{
        self, WebViewRequest, WebViewResponse, WebViewScript, WebViewSession, WebViewWait,
        WebViewWaitUntil,
    },
    client::Client,
    html::{self, ElementRef, Html},
    model::{
        CatalogItem, FilterDefinition, ImageRequest, ImageRequestContext, NovelChapter,
        NovelContentBlock, NovelText, OptionItem, Paged, UrlResolveResult,
    },
    Error, NovelSource, Result,
};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashSet;
use url::Url;

#[cfg(target_arch = "wasm32")]
const SOURCE_ID: &str = "chrysanthemumgarden";
const BASE_URL: &str = "https://chrysanthemumgarden.com";
const CHALLENGE_TIMEOUT_MS: u64 = 45_000;

pub struct ChrysanthemumGardenSource {
    client: Client,
}

impl Default for ChrysanthemumGardenSource {
    fn default() -> Self {
        Self {
            client: Client::browser().cookies_for(BASE_URL),
        }
    }
}

impl ChrysanthemumGardenSource {
    fn document(&self, url: &str) -> Result<(Html, String)> {
        let response = self.client.get(url).send()?.error_for_status()?;
        Ok((html::document(response.text()?), url.to_owned()))
    }

    fn catalog_url(page: u32, category: Option<&str>) -> String {
        let page = page.max(1);
        let root = match category.filter(|value| !value.is_empty()) {
            Some("completed") => format!("{BASE_URL}/tag/completed/"),
            Some(category) => format!("{BASE_URL}/genre/{category}/"),
            None => format!("{BASE_URL}/books/"),
        };
        if page == 1 {
            root
        } else {
            format!("{root}page/{page}/")
        }
    }

    fn search_url(query: &str, page: u32) -> Result<String> {
        let page = page.max(1);
        let path = if page == 1 {
            BASE_URL.to_owned()
        } else {
            format!("{BASE_URL}/page/{page}/")
        };
        let mut url = Url::parse(&path).map_err(|error| Error::new(error.to_string()))?;
        url.query_pairs_mut().append_pair("s", query.trim());
        Ok(url.to_string())
    }

    fn parse_catalog(document: &Html, page_url: &str) -> Result<Paged<CatalogItem>> {
        let articles = selector("article.lb_novel")?;
        let titles = selector("h2.novel-title > a")?;
        let covers = selector("div.novel-cover img")?;
        let genres = selector("div.series-genres > a")?;
        let next = selector("a.next.page-numbers")?;
        let mut entries = Vec::new();

        for article in document.select(&articles) {
            let article_genres = article
                .select(&genres)
                .map(html::text)
                .map(|value| normalize_space(&value))
                .collect::<Vec<_>>();
            if article_genres
                .iter()
                .any(|value| value.eq_ignore_ascii_case("manhua"))
            {
                continue;
            }
            let Some(anchor) = article.select(&titles).next() else {
                continue;
            };
            let Some(href) = attr(anchor, "href") else {
                continue;
            };
            let title = normalize_space(&html::text(anchor));
            if title.is_empty() {
                continue;
            }
            let url = absolute_url(page_url, &href)?;
            let mut item = CatalogItem::new(url.clone(), title);
            item.url = Some(url.clone());
            item.language = Some("en".into());
            item.tags = article_genres;
            item.cover = article
                .select(&covers)
                .next()
                .and_then(lazy_image_url)
                .map(|cover| absolute_url(page_url, &cover))
                .transpose()?
                .map(|cover| image(&cover, &url));
            entries.push(item);
        }

        Ok(Paged::new(entries, document.select(&next).next().is_some()))
    }

    fn parse_latest(document: &Html) -> Result<Paged<CatalogItem>> {
        let links = selector("a.release-novel")?;
        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        for anchor in document.select(&links) {
            let Some(href) = attr(anchor, "href") else {
                continue;
            };
            let url = absolute_url(BASE_URL, &href)?;
            if !seen.insert(url.clone()) {
                continue;
            }
            let title = normalize_space(&html::text(anchor));
            if title.is_empty() {
                continue;
            }
            let mut item = CatalogItem::new(url.clone(), title);
            item.url = Some(url);
            item.language = Some("en".into());
            entries.push(item);
        }
        Ok(Paged::new(entries, false))
    }

    fn parse_details(document: &Html, page_url: &str) -> Result<CatalogItem> {
        let title = first_text(document, "h1.entry-title, h1.novel-title")?
            .ok_or_else(|| Error::new("Chrysanthemum Garden novel has no title"))?;
        let mut item = CatalogItem::new(page_url, title);
        item.url = Some(page_url.to_owned());
        item.language = Some("en".into());
        item.initialized = true;

        if let Some(cover) =
            first_element(document, "div.novel-cover img")?.and_then(lazy_image_url)
        {
            let cover = absolute_url(page_url, &cover)?;
            item.cover = Some(image(&cover, page_url));
        }

        let paragraphs = texts(document, "div.entry-content > p")?;
        item.description = (!paragraphs.is_empty()).then(|| paragraphs.join("\n\n"));

        if let Some(info) = first_element(document, "div.novel-info")? {
            let author = Regex::new(r"(?i)Author:\s*([^<]+)<br\s*/?>")
                .map_err(|error| Error::new(error.to_string()))?
                .captures(&info.inner_html())
                .and_then(|captures| captures.get(1))
                .map(|value| normalize_space(value.as_str()))
                .filter(|value| !value.is_empty());
            item.authors = author.into_iter().collect();
        }

        item.tags = texts(document, "div.series-genres > a, a.series-tag")?
            .into_iter()
            .map(|value| strip_tag_count(&value))
            .filter(|value| !value.is_empty())
            .collect();
        item.tags.sort();
        item.tags.dedup();
        item.status = Some(json!(status_for(&item.tags)));
        item.content_rating = Some(content_rating(&item.tags).into());
        Ok(item)
    }

    fn parse_chapters(document: &Html) -> Result<Vec<NovelChapter>> {
        let links = selector("div.chapter-item > a")?;
        let mut chapters = Vec::new();
        for anchor in document.select(&links) {
            let Some(href) = attr(anchor, "href") else {
                continue;
            };
            let title = normalize_space(&html::text(anchor));
            let url = absolute_url(BASE_URL, &href)?;
            chapters.push(NovelChapter {
                key: url.clone(),
                title: (!title.is_empty()).then_some(title.clone()),
                chapter_number: chapter_number(&title),
                url: Some(url),
                language: Some("en".into()),
                source_order: Some(chapters.len() as i32),
                ..NovelChapter::default()
            });
        }
        require(
            (!chapters.is_empty()).then_some(()),
            "Chrysanthemum Garden novel has no readable chapters",
        )?;
        Ok(chapters)
    }

    fn chapter_text(&self, url: &str) -> Result<NovelText> {
        let response: WebViewResponse = browser::open(&WebViewRequest {
            url: url.to_owned(),
            cookie_url: Some(BASE_URL.to_owned()),
            session: Some(WebViewSession {
                id: "chrysanthemum-garden-reader".to_owned(),
                ..WebViewSession::default()
            }),
            wait_for: Some(WebViewWait::Script {
                script: r##"document.readyState === "complete" &&
                    document.fonts && document.fonts.status === "loaded" &&
                    !!document.querySelector("#novel-content") &&
                    document.title !== "Just a moment..." &&
                    !document.getElementById("challenge-error-title") &&
                    !document.querySelector('.cf-turnstile, [name="cf-turnstile-response"]')"##
                    .to_owned(),
            }),
            wait_until: Some(WebViewWaitUntil::LoadFinished),
            headers: vec![
                ("Referer".to_owned(), BASE_URL.to_owned()),
                (
                    "Accept".to_owned(),
                    "text/html,application/xhtml+xml".to_owned(),
                ),
                ("Accept-Language".to_owned(), "en-US,en;q=0.9".to_owned()),
            ],
            timeout_ms: Some(CHALLENGE_TIMEOUT_MS),
            return_html: false,
            scripts: vec![WebViewScript {
                id: Some("chrysanthemum-chapter".to_owned()),
                script: CHAPTER_SCRIPT.to_owned(),
                run_at: None,
            }],
            ..WebViewRequest::default()
        })?;
        let payload = response
            .script_results
            .iter()
            .find(|result| result.id.as_deref() == Some("chrysanthemum-chapter"))
            .and_then(|result| result.value.as_ref())
            .ok_or_else(|| Error::new("Chrysanthemum Garden browser returned no chapter"))?;
        parse_browser_text(payload, &response.final_url)
    }

    fn item_url(item: &CatalogItem) -> Result<String> {
        let candidate = item.url.as_deref().unwrap_or(&item.key);
        let url = absolute_url(BASE_URL, candidate)?;
        require(
            is_item_url(&url).then_some(()),
            "invalid Chrysanthemum Garden novel URL",
        )?;
        Ok(url)
    }
}

impl NovelSource for ChrysanthemumGardenSource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.listing("popular", page, &json!({}))
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        if page > 1 {
            return Ok(Paged::new(Vec::new(), false));
        }
        let (document, _) = self.document(&format!("{BASE_URL}/"))?;
        Self::parse_latest(&document)
    }

    fn listing(&mut self, listing: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        match listing {
            "latest" => self.latest(page),
            "popular" => {
                let category = filters
                    .get("category")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                let url = Self::catalog_url(page, category);
                let (document, final_url) = self.document(&url)?;
                Self::parse_catalog(&document, &final_url)
            }
            _ => Err(Error::new(format!(
                "unknown Chrysanthemum Garden listing {listing:?}"
            ))),
        }
    }

    fn search(&mut self, query: &str, page: u32, _filters: &Value) -> Result<Paged<CatalogItem>> {
        if query.trim().is_empty() {
            return self.popular(page);
        }
        let url = Self::search_url(query, page)?;
        let (document, final_url) = self.document(&url)?;
        Self::parse_catalog(&document, &final_url)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let url = Self::item_url(&item)?;
        let (document, final_url) = self.document(&url)?;
        Self::parse_details(&document, &final_url)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        let url = Self::item_url(&item)?;
        let (document, _) = self.document(&url)?;
        Self::parse_chapters(&document)
    }

    fn text(&mut self, _item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let candidate = chapter.url.as_deref().unwrap_or(&chapter.key);
        let url = absolute_url(BASE_URL, candidate)?;
        require(
            is_chapter_url(&url).then_some(()),
            "invalid Chrysanthemum Garden chapter URL",
        )?;
        self.chapter_text(&url)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![FilterDefinition::Select {
            id: "category".into(),
            name: "Category".into(),
            options: CATEGORIES
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
        let Ok(url) = Url::parse(candidate) else {
            return Ok(None);
        };
        if url.host_str() != Some("chrysanthemumgarden.com") {
            return Ok(None);
        }
        if is_chapter_url(candidate) {
            return Ok(Some(UrlResolveResult {
                novel_chapter: Some(NovelChapter {
                    key: candidate.into(),
                    url: Some(candidate.into()),
                    language: Some("en".into()),
                    ..NovelChapter::default()
                }),
                ..UrlResolveResult::default()
            }));
        }
        if is_item_url(candidate) {
            let mut item = CatalogItem::new(candidate, "");
            item.url = Some(candidate.into());
            item.language = Some("en".into());
            return Ok(Some(UrlResolveResult {
                item: Some(item),
                ..UrlResolveResult::default()
            }));
        }
        Ok(None)
    }
}

fn parse_browser_text(payload: &Value, page_url: &str) -> Result<NovelText> {
    let unresolved = payload
        .get("unresolvedFonts")
        .and_then(Value::as_array)
        .map(|values| values.len())
        .unwrap_or(0);
    require(
        (unresolved == 0).then_some(()),
        "Chrysanthemum Garden protected text font could not be decoded",
    )?;
    let rendered = payload
        .get("html")
        .and_then(Value::as_str)
        .map(|value| sanitize_html(value, page_url))
        .transpose()?
        .filter(|value| {
            !normalize_space(&html::text(html::fragment(value).root_element())).is_empty()
        })
        .ok_or_else(|| Error::new("Chrysanthemum Garden chapter has no readable text"))?;
    let title = payload
        .get("title")
        .and_then(Value::as_str)
        .map(normalize_space)
        .filter(|value| !value.is_empty());
    Ok(NovelText {
        html: Some(rendered.clone()),
        title,
        base_url: Some(page_url.to_owned()),
        image_context: Some(ImageRequestContext {
            headers: [("Referer".to_owned(), page_url.to_owned())]
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

fn lazy_image_url(element: ElementRef<'_>) -> Option<String> {
    attr(element, "data-breeze")
        .or_else(|| attr(element, "data-src"))
        .or_else(|| attr(element, "src"))
        .filter(|value| !value.starts_with("data:"))
}

fn image(url: &str, referer: &str) -> ImageRequest {
    ImageRequest::get(url).header("Referer", referer)
}

fn first_element<'a>(document: &'a Html, query: &str) -> Result<Option<ElementRef<'a>>> {
    let query = selector(query)?;
    Ok(document.select(&query).next())
}

fn first_text(document: &Html, query: &str) -> Result<Option<String>> {
    Ok(first_element(document, query)?
        .map(html::text)
        .map(|value| normalize_space(&value))
        .filter(|value| !value.is_empty()))
}

fn texts(document: &Html, query: &str) -> Result<Vec<String>> {
    let query = selector(query)?;
    Ok(document
        .select(&query)
        .map(html::text)
        .map(|value| normalize_space(&value))
        .filter(|value| !value.is_empty())
        .collect())
}

fn strip_tag_count(value: &str) -> String {
    Regex::new(r"\s*\(\d+\)\s*$")
        .expect("valid tag count regex")
        .replace(value, "")
        .trim()
        .to_owned()
}

fn status_for(tags: &[String]) -> &'static str {
    if tags.iter().any(|tag| tag.eq_ignore_ascii_case("completed")) {
        "completed"
    } else if tags.iter().any(|tag| tag.eq_ignore_ascii_case("dropped")) {
        "cancelled"
    } else {
        "ongoing"
    }
}

fn content_rating(tags: &[String]) -> &'static str {
    if tags.iter().any(|tag| {
        matches!(
            tag.to_ascii_lowercase().as_str(),
            "adult" | "mature" | "smut" | "explicit sexual content"
        )
    }) {
        "adult"
    } else {
        "suggestive"
    }
}

fn chapter_number(value: &str) -> Option<f32> {
    Regex::new(r"(?i)\b(?:ch(?:apter)?)\s*([0-9]+(?:\.[0-9]+)?)")
        .ok()?
        .captures(value)?
        .get(1)?
        .as_str()
        .parse()
        .ok()
}

fn path_segments(value: &str) -> Option<Vec<String>> {
    let url = Url::parse(value).ok()?;
    Some(
        url.path_segments()?
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect(),
    )
}

fn is_item_url(value: &str) -> bool {
    path_segments(value).is_some_and(|parts| parts.len() == 2 && parts[0] == "novel-tl")
}

fn is_chapter_url(value: &str) -> bool {
    path_segments(value).is_some_and(|parts| parts.len() == 3 && parts[0] == "novel-tl")
}

fn sanitize_html(value: &str, base: &str) -> Result<String> {
    let mut rendered = value.to_owned();
    for tag in [
        "script", "style", "iframe", "object", "embed", "form", "button",
    ] {
        let pattern = Regex::new(&format!(
            r"(?is)<{tag}\b[^>]*>.*?</{tag}\s*>|<{tag}\b[^>]*/?>"
        ))
        .map_err(|error| Error::new(error.to_string()))?;
        rendered = pattern.replace_all(&rendered, "").into_owned();
    }
    let event = Regex::new(r#"(?i)\s+on[a-z]+\s*=\s*(?:\"[^\"]*\"|'[^']*')"#)
        .map_err(|error| Error::new(error.to_string()))?;
    rendered = event.replace_all(&rendered, "").into_owned();
    let dangerous =
        Regex::new(r#"(?i)\s+(src|href|xlink:href)=\"\s*(?:javascript:|data:text/html)[^\"]*\""#)
            .map_err(|error| Error::new(error.to_string()))?;
    rendered = dangerous.replace_all(&rendered, "").into_owned();
    let relative = Regex::new(r#"(?i)(src|href)=\"(/[^"]*)\""#)
        .map_err(|error| Error::new(error.to_string()))?;
    Ok(relative
        .replace_all(&rendered, |captures: &regex::Captures<'_>| {
            let absolute = absolute_url(base, &captures[2]).unwrap_or_else(|_| captures[2].into());
            format!("{}=\"{}\"", &captures[1], absolute)
        })
        .into_owned())
}

const CATEGORIES: &[(&str, &str)] = &[
    ("All", ""),
    ("Quick Transmigration", "quick-transmigration"),
    ("Unlimited Flow", "unlimited-flow"),
    ("Modern", "modern-modern"),
    ("Apocalypse", "apocalypse"),
    ("Entertainment", "entertainment"),
    ("Gaming", "gaming"),
    ("School", "school"),
    ("Modern Fantasy", "modern-fantasy"),
    ("Cultivation", "cultivation"),
    ("Historical", "historical"),
    ("Interstellar", "interstellar"),
    ("Western Fantasy", "western-fantasy"),
    ("Original Novel", "original-novel"),
    ("Short", "short"),
    ("Audio", "audio"),
    ("Teaser", "teaser"),
    ("Dropped", "dropped"),
    ("Completed", "completed"),
];

// The site substitutes Latin glyphs with a randomized Open Sans font. This
// script compares those rendered glyphs with the page's canonical Open Sans,
// decodes the clone, and returns ordinary selectable text to the host reader.
const CHAPTER_SCRIPT: &str = r##"(() => {
  const root = document.querySelector("#novel-content");
  if (!root) return { html: "", title: "", unresolvedFonts: ["missing-content"] };
  const letters = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
  const paint = (font, character) => {
    const canvas = document.createElement("canvas");
    canvas.width = 100;
    canvas.height = 70;
    const context = canvas.getContext("2d", { willReadFrequently: true });
    context.clearRect(0, 0, canvas.width, canvas.height);
    context.fillStyle = "#000";
    context.font = `40px "${font}"`;
    context.textBaseline = "alphabetic";
    context.fillText(character, 5, 50);
    const rgba = context.getImageData(0, 0, canvas.width, canvas.height).data;
    const pixels = new Uint8Array(canvas.width * canvas.height);
    for (let source = 3, target = 0; source < rgba.length; source += 4, target++) {
      pixels[target] = rgba[source];
    }
    return {
      pixels,
      width: context.measureText(character).width
    };
  };
  const references = {};
  for (const character of letters) {
    references[character] = paint("Open Sans", character);
  }
  const fontNames = Array.from(root.querySelectorAll("span[style*='font-family']"))
    .map(span => span.style.fontFamily.replace(/[\"']/g, "").trim())
    .filter((font, index, values) => font && values.indexOf(font) === index);
  const maps = {};
  const unresolvedFonts = [];
  for (const font of fontNames) {
    const mapping = {};
    for (const cipher of letters) {
      const rendered = paint(font, cipher);
      const candidates = Array.from(letters)
        .map(plain => {
          const reference = references[plain];
          let difference = Math.abs(rendered.width - reference.width) * 500;
          for (let index = 0; index < rendered.pixels.length; index++) {
            difference += Math.abs(rendered.pixels[index] - reference.pixels[index]);
          }
          return { plain, difference };
        })
        .sort((left, right) => left.difference - right.difference);
      if (candidates.length > 1 && candidates[0].difference < candidates[1].difference) {
        mapping[cipher] = candidates[0].plain;
      }
    }
    if (Object.keys(mapping).length !== letters.length ||
        new Set(Object.values(mapping)).size !== letters.length) {
      unresolvedFonts.push(font);
    }
    maps[font] = mapping;
  }
  const clone = root.cloneNode(true);
  for (const span of clone.querySelectorAll("span[style*='font-family']")) {
    const font = span.style.fontFamily.replace(/[\"']/g, "").trim();
    const mapping = maps[font] || {};
    const walker = document.createTreeWalker(span, NodeFilter.SHOW_TEXT);
    const nodes = [];
    while (walker.nextNode()) nodes.push(walker.currentNode);
    for (const node of nodes) {
      node.textContent = Array.from(node.textContent || "", character => mapping[character] || character).join("");
    }
  }
  clone.querySelectorAll("script,style,noscript,iframe,object,embed,form,button,svg,link").forEach(node => node.remove());
  clone.querySelectorAll("[style]").forEach(node => {
    const style = node.getAttribute("style") || "";
    if (/height\s*:\s*1px/i.test(style) && /overflow\s*:\s*hidden/i.test(style)) node.remove();
    else node.removeAttribute("style");
  });
  clone.querySelectorAll("img").forEach(image => {
    const lazy = image.getAttribute("data-breeze") || image.getAttribute("data-src");
    if (lazy) image.setAttribute("src", lazy);
    image.removeAttribute("srcset");
  });
  const title = document.querySelector(".chapter-title")?.textContent?.trim() ||
    document.querySelector("h1.entry-title")?.textContent?.trim() || "";
  return { html: clone.innerHTML, title, unresolvedFonts };
})()"##;

#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel(SOURCE_ID, ChrysanthemumGardenSource::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_catalog_and_skips_manhua() {
        let document = html::document(include_str!("../tests/fixtures/catalog.html"));
        let page = ChrysanthemumGardenSource::parse_catalog(
            &document,
            "https://chrysanthemumgarden.com/books/",
        )
        .unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].title, "Fixture Novel");
        assert_eq!(
            page.entries[0].cover.as_ref().unwrap().url,
            "https://chrysanthemumgarden.com/wp-content/uploads/fixture.jpg"
        );
        assert!(page.has_next_page);
    }

    #[test]
    fn parses_details_and_chapters() {
        let document = html::document(include_str!("../tests/fixtures/details.html"));
        let item = ChrysanthemumGardenSource::parse_details(
            &document,
            "https://chrysanthemumgarden.com/novel-tl/fixture/",
        )
        .unwrap();
        assert_eq!(item.title, "Fixture Novel");
        assert_eq!(item.authors, vec!["Fixture Author"]);
        assert_eq!(item.status, Some(json!("completed")));
        assert!(item.tags.contains(&"BL".to_owned()));
        let chapters = ChrysanthemumGardenSource::parse_chapters(&document).unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[1].chapter_number, Some(2.5));
        assert_eq!(chapters[1].source_order, Some(1));
    }

    #[test]
    fn accepts_decoded_browser_text_and_sanitizes_it() {
        let text = parse_browser_text(
            &json!({
                "html": "<p>Readable protected text.</p><script>bad()</script><img src=\"/image.jpg\" onerror=\"bad()\">",
                "title": "Chapter 1",
                "unresolvedFonts": []
            }),
            "https://chrysanthemumgarden.com/novel-tl/fixture/chapter-1/",
        )
        .unwrap();
        let rendered = text.html.unwrap();
        assert!(rendered.contains("Readable protected text."));
        assert!(rendered.contains("https://chrysanthemumgarden.com/image.jpg"));
        assert!(!rendered.contains("script"));
        assert!(!rendered.contains("onerror"));
    }

    #[test]
    fn rejects_unresolved_protected_fonts() {
        let error = parse_browser_text(
            &json!({"html": "<p>ciphertext</p>", "unresolvedFonts": ["random-font"]}),
            "https://chrysanthemumgarden.com/novel-tl/fixture/chapter-1/",
        )
        .unwrap_err();
        assert!(error.to_string().contains("could not be decoded"));
    }

    #[test]
    fn resolves_only_supported_urls() {
        assert!(is_item_url(
            "https://chrysanthemumgarden.com/novel-tl/fixture/"
        ));
        assert!(is_chapter_url(
            "https://chrysanthemumgarden.com/novel-tl/fixture/chapter-1/"
        ));
        assert!(!is_item_url("https://chrysanthemumgarden.com/books/"));
    }
}
