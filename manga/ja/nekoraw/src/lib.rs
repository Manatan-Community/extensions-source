use std::collections::{BTreeMap, BTreeSet};

use manatan_sdk::{
    client::{Client, BROWSER_USER_AGENT},
    CatalogItem, Error, FilterDefinition, ImageRequest, MangaChapter, MangaPage, MangaSource,
    OptionItem, PageContent, Paged, Result, UrlResolveResult,
};
use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};
use url::Url;

const BASE_URL: &str = "https://kuronavi.one";
const REFERER: &str = "https://kuronavi.one/";
const REQUEST_LIMIT_MS: u32 = 250;
const LANGUAGE: &str = "ja";
const CONTENT_RATING: &str = "adult";

pub struct NekorawSource {
    client: Client,
}

impl Default for NekorawSource {
    fn default() -> Self {
        Self {
            client: Client::browser()
                .cookies_for(BASE_URL)
                .header("Referer", REFERER),
        }
    }
}

impl NekorawSource {
    fn document(&self, url: &str) -> Result<Html> {
        let response = self
            .client
            .get(url)
            .rate_limit("nekoraw", REQUEST_LIMIT_MS)
            .send()?
            .error_for_status()?;
        Ok(Html::parse_document(response.text()?))
    }

    fn listing_page(&self, page: u32, sort: Option<&str>) -> Result<Paged<CatalogItem>> {
        let url = search_url("", page, &Value::Null, sort)?;
        parse_catalog(&self.document(&url)?, page)
    }
}

impl MangaSource for NekorawSource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let mut url = Url::parse(&format!("{BASE_URL}/hot")).map_err(url_error)?;
        if page > 1 {
            url.query_pairs_mut().append_pair("page", &page.to_string());
        }
        parse_catalog(&self.document(url.as_str())?, page)
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.listing_page(page, Some("-1"))
    }

    fn listing(
        &mut self,
        listing: &str,
        page: u32,
        _filters: &Value,
    ) -> Result<Paged<CatalogItem>> {
        match listing {
            "popular" => self.popular(page),
            "latest" => self.latest(page),
            "top" => self.listing_page(page, Some("10")),
            other => Err(Error::new(format!("unknown NekoRaw listing {other:?}"))),
        }
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        let query = query.trim();
        if (query.starts_with("https://") || query.starts_with("http://"))
            && self.handle_url(query)?.is_some()
        {
            let resolved = self
                .handle_url(query)?
                .and_then(|result| result.item)
                .into_iter()
                .collect();
            return Ok(Paged::new(resolved, false));
        }
        let url = search_url(query, page, filters, None)?;
        parse_catalog(&self.document(&url)?, page)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let slug = manga_slug(item.url.as_deref().unwrap_or(&item.key))
            .ok_or_else(|| Error::new("NekoRaw item has no manga slug"))?;
        let url = manga_url(&slug);
        let parsed = parse_details(&self.document(&url)?, &slug, &url)?;
        Ok(parsed)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<MangaChapter>> {
        let slug = manga_slug(item.url.as_deref().unwrap_or(&item.key))
            .ok_or_else(|| Error::new("NekoRaw item has no manga slug"))?;
        parse_chapters(&self.document(&manga_url(&slug))?, &slug)
    }

    fn pages(&mut self, item: CatalogItem, chapter: MangaChapter) -> Result<Vec<MangaPage>> {
        let slug = manga_slug(item.url.as_deref().unwrap_or(&item.key))
            .ok_or_else(|| Error::new("NekoRaw item has no manga slug"))?;
        let chapter_url =
            canonical_chapter_url(&slug, chapter.url.as_deref().unwrap_or(&chapter.key))
                .ok_or_else(|| Error::new("NekoRaw chapter has no chapter key"))?;
        parse_pages(&self.document(&chapter_url)?, &chapter_url)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(filter_definitions())
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        let slug = manga_slug(item.url.as_deref().unwrap_or(&item.key))
            .ok_or_else(|| Error::new("NekoRaw item has no manga slug"))?;
        Ok(Some(manga_url(&slug)))
    }

    fn chapter_url(
        &mut self,
        item: &CatalogItem,
        chapter: &MangaChapter,
    ) -> Result<Option<String>> {
        let slug = manga_slug(item.url.as_deref().unwrap_or(&item.key))
            .ok_or_else(|| Error::new("NekoRaw item has no manga slug"))?;
        Ok(canonical_chapter_url(
            &slug,
            chapter.url.as_deref().unwrap_or(&chapter.key),
        ))
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let parsed = Url::parse(candidate).map_err(url_error)?;
        if !matches!(
            parsed.host_str(),
            Some("kuronavi.one" | "comiraw.net" | "nekoraw.blog")
        ) {
            return Ok(None);
        }
        let Some(slug) = manga_slug(parsed.path()) else {
            return Ok(None);
        };
        let item_url = manga_url(&slug);
        let item = CatalogItem {
            key: slug.clone(),
            url: Some(item_url),
            language: Some(LANGUAGE.to_owned()),
            content_rating: Some(CONTENT_RATING.to_owned()),
            viewer: Some(json!("rtl")),
            ..CatalogItem::default()
        };
        let mut result = UrlResolveResult {
            item: Some(item),
            ..UrlResolveResult::default()
        };
        if let Some(chapter_key) = chapter_key(parsed.path(), &slug) {
            let url = format!("{BASE_URL}/manga/{slug}/{chapter_key}");
            let manga_chapter = MangaChapter {
                key: chapter_key.clone(),
                chapter_number: chapter_number(&chapter_key),
                language: Some(LANGUAGE.to_owned()),
                url: Some(url),
                ..MangaChapter::default()
            };
            result.chapter_key = Some(chapter_key);
            result.manga_chapter = Some(manga_chapter);
        }
        Ok(Some(result))
    }
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(
    manatan_sdk::Extension::new().manga("nekoraw", NekorawSource::default())
);

fn parse_catalog(document: &Html, current_page: u32) -> Result<Paged<CatalogItem>> {
    let anchors = select("a[href*='/manga/']")?;
    let image_selector = select("img")?;
    let title_selector = select("h3")?;
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();

    for anchor in document.select(&anchors) {
        let Some(href) = anchor.value().attr("href") else {
            continue;
        };
        let Some(slug) = manga_slug(href) else {
            continue;
        };
        if chapter_key(href, &slug).is_some() || !seen.insert(slug.clone()) {
            continue;
        }
        let title = anchor
            .select(&title_selector)
            .next()
            .map(element_text)
            .filter(|value| !value.is_empty())
            .or_else(|| attr(&anchor, "title"))
            .or_else(|| {
                let text = element_text(anchor);
                (!text.is_empty()).then_some(text)
            });
        let Some(title) = title else {
            seen.remove(&slug);
            continue;
        };
        let cover = anchor
            .select(&image_selector)
            .next()
            .and_then(image_url)
            .map(image_request);
        entries.push(CatalogItem {
            key: slug.clone(),
            title,
            url: Some(manga_url(&slug)),
            cover,
            language: Some(LANGUAGE.to_owned()),
            content_rating: Some(CONTENT_RATING.to_owned()),
            viewer: Some(json!("rtl")),
            ..CatalogItem::default()
        });
    }

    Ok(Paged::new(
        entries,
        has_page_link(document, current_page.saturating_add(1))?,
    ))
}

fn parse_details(document: &Html, slug: &str, url: &str) -> Result<CatalogItem> {
    let title = first_text(document, "h1")?
        .ok_or_else(|| Error::new("NekoRaw details page has no title"))?;
    let cover = first_attr(document, "meta[itemprop='image']", "content")?
        .or(first_attr(
            document,
            "meta[property='og:image']",
            "content",
        )?)
        .map(|value| absolute_url(&value))
        .transpose()?
        .map(image_request);
    let description = first_attr(document, "meta[property='og:description']", "content")?
        .or(first_attr(document, "meta[name='description']", "content")?)
        .map(|value| clean_text(&value))
        .filter(|value| !value.is_empty());
    let tags = parse_tags(document)?;
    let status = parse_detail_status(document)?;

    Ok(CatalogItem {
        key: slug.to_owned(),
        title,
        url: Some(url.to_owned()),
        cover,
        description,
        tags,
        status: Some(json!(status)),
        initialized: true,
        language: Some(LANGUAGE.to_owned()),
        content_rating: Some(CONTENT_RATING.to_owned()),
        viewer: Some(json!("rtl")),
        ..CatalogItem::default()
    })
}

fn parse_detail_status(document: &Html) -> Result<&'static str> {
    let selector = select("#main-content span")?;
    Ok(document
        .select(&selector)
        .map(element_text)
        .find_map(|text| {
            let status = parse_status(&text);
            (status != "unknown").then_some(status)
        })
        .unwrap_or("unknown"))
}

fn parse_tags(document: &Html) -> Result<Vec<String>> {
    let selector = select("a[href*='search/manga?genre=']")?;
    let mut seen = BTreeSet::new();
    Ok(document
        .select(&selector)
        .map(element_text)
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.clone()))
        .collect())
}

fn parse_chapters(document: &Html, slug: &str) -> Result<Vec<MangaChapter>> {
    let selector = select("a[href*='/manga/'][href*='/chapter-']")?;
    let mut seen = BTreeSet::new();
    let mut chapters = Vec::new();
    for anchor in document.select(&selector) {
        let Some(href) = anchor.value().attr("href") else {
            continue;
        };
        let Some(key) = chapter_key(href, slug) else {
            continue;
        };
        if !seen.insert(key.clone()) {
            continue;
        }
        let text = element_text(anchor);
        chapters.push(MangaChapter {
            key: key.clone(),
            title: chapter_title(&text),
            chapter_number: chapter_number(&key).or_else(|| number_in_text(&text)),
            language: Some(LANGUAGE.to_owned()),
            url: Some(format!("{BASE_URL}/manga/{slug}/{key}")),
            source_order: Some(chapters.len() as i32),
            ..MangaChapter::default()
        });
    }
    if chapters.is_empty() {
        return Err(Error::new("NekoRaw details page has no chapters"));
    }
    Ok(chapters)
}

fn parse_pages(document: &Html, chapter_url: &str) -> Result<Vec<MangaPage>> {
    let selector = select("div.page-chapter img")?;
    let mut seen = BTreeSet::new();
    let mut pages = Vec::new();
    for image in document.select(&selector) {
        let Some(url) = image_url(image) else {
            continue;
        };
        let url = absolute_url(&url)?;
        if !seen.insert(url.clone()) {
            continue;
        }
        let mut context = BTreeMap::new();
        context.insert("Referer".to_owned(), chapter_url.to_owned());
        context.insert("User-Agent".to_owned(), BROWSER_USER_AGENT.to_owned());
        pages.push(MangaPage {
            content: PageContent::Url {
                url,
                context: Some(context),
            },
            description: Some(format!("Page {}", pages.len() + 1)),
            ..MangaPage::default()
        });
    }
    if pages.is_empty() {
        return Err(Error::new("NekoRaw chapter page has no images"));
    }
    Ok(pages)
}

fn search_url(
    query: &str,
    page: u32,
    filters: &Value,
    forced_sort: Option<&str>,
) -> Result<String> {
    let mut url = Url::parse(&format!("{BASE_URL}/search/manga")).map_err(url_error)?;
    {
        let mut pairs = url.query_pairs_mut();
        if !query.trim().is_empty() {
            pairs.append_pair("keyword", query.trim());
        }
        if let Some(genre) = selected(filters, "genre").filter(|value| !value.is_empty()) {
            pairs.append_pair("genre", genre);
        }
        if let Some(status) = selected(filters, "status").filter(|value| *value != "-1") {
            pairs.append_pair("status", status);
        }
        if let Some(sort) = forced_sort.or_else(|| selected(filters, "sort")) {
            if sort != "-1" || forced_sort.is_some() {
                pairs.append_pair("sort", sort);
            }
        }
        if page > 1 {
            pairs.append_pair("page", &page.to_string());
        }
    }
    Ok(url.to_string())
}

fn selected<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn filter_definitions() -> Vec<FilterDefinition> {
    vec![
        FilterDefinition::Select {
            id: "genre".to_owned(),
            name: "Genre".to_owned(),
            options: vec![
                option("All", ""),
                option("Full Color", "Full Color"),
                option("Action", "Action"),
                option("Adventure", "Adventure"),
                option("Business", "Business"),
                option("Comedy", "Comedy"),
                option("Cooking", "Cooking"),
                option("Drama", "Drama"),
                option("Ecchi", "Ecchi"),
                option("Fantasy", "Fantasy"),
                option("Harem", "Harem"),
                option("Historical", "Historical"),
                option("Horror", "Horror"),
                option("Isekai", "Isekai"),
                option("Martial Arts", "Martial Arts"),
                option("Mature", "Mature"),
                option("Mecha", "Mecha"),
                option("Medical", "Medical"),
                option("Military", "Military"),
                option("Music", "Music"),
                option("Mystery", "Mystery"),
                option("Psychological", "Psychological"),
                option("Romance", "Romance"),
                option("School", "School"),
                option("Sci-Fi", "Sci-Fi"),
                option("Seinen", "Seinen"),
                option("Shoujo", "Shoujo"),
            ],
            default_index: 0,
        },
        FilterDefinition::Select {
            id: "status".to_owned(),
            name: "Status".to_owned(),
            options: vec![
                option("All", "-1"),
                option("Ongoing", "0"),
                option("Completed", "1"),
            ],
            default_index: 0,
        },
        FilterDefinition::Select {
            id: "sort".to_owned(),
            name: "Sort by".to_owned(),
            options: vec![
                option("Last updated", "-1"),
                option("Latest", "15"),
                option("Top", "10"),
                option("Top month", "11"),
                option("Top week", "12"),
                option("Top day", "13"),
                option("Most followed", "20"),
                option("Chapter count", "30"),
            ],
            default_index: 0,
        },
    ]
}

fn option(label: &str, value: &str) -> OptionItem {
    OptionItem {
        label: label.to_owned(),
        value: value.to_owned(),
    }
}

fn manga_url(slug: &str) -> String {
    format!("{BASE_URL}/manga/{slug}")
}

fn manga_slug(candidate: &str) -> Option<String> {
    let path = candidate_path(candidate);
    let mut segments = path.trim_matches('/').split('/');
    if segments.next()? != "manga" {
        return None;
    }
    let slug = segments.next()?.trim();
    (!slug.is_empty()).then(|| slug.to_owned())
}

fn chapter_key(candidate: &str, slug: &str) -> Option<String> {
    let path = candidate_path(candidate);
    let mut segments = path.trim_matches('/').split('/');
    if segments.next()? != "manga" || segments.next()? != slug {
        return None;
    }
    let key = segments.next()?.trim();
    (key.starts_with("chapter-") && segments.next().is_none()).then(|| key.to_owned())
}

fn canonical_chapter_url(slug: &str, candidate: &str) -> Option<String> {
    let key = if candidate.starts_with("chapter-") {
        candidate.trim_matches('/').to_owned()
    } else {
        chapter_key(candidate, slug)?
    };
    Some(format!("{BASE_URL}/manga/{slug}/{key}"))
}

fn candidate_path(candidate: &str) -> &str {
    let path = candidate
        .split_once("://")
        .and_then(|(_, rest)| rest.find('/').map(|index| &rest[index..]))
        .unwrap_or(candidate);
    path.split(['?', '#']).next().unwrap_or(path)
}

fn chapter_number(key: &str) -> Option<f32> {
    key.strip_prefix("chapter-")?.parse().ok()
}

fn number_in_text(text: &str) -> Option<f32> {
    let start = text.find(|character: char| character.is_ascii_digit())?;
    let number = text[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect::<String>();
    number.parse().ok()
}

fn chapter_title(text: &str) -> Option<String> {
    let text = clean_text(text);
    if text.is_empty() || number_in_text(&text).is_some() {
        None
    } else {
        Some(text)
    }
}

fn parse_status(text: &str) -> &'static str {
    if text.contains("完結") {
        "completed"
    } else if text.contains("休載") {
        "hiatus"
    } else if text.contains("連載") || text.contains("更新中") {
        "ongoing"
    } else {
        "unknown"
    }
}

fn image_request(url: String) -> ImageRequest {
    ImageRequest::get(url)
        .cookies_for(BASE_URL)
        .header("Referer", REFERER)
        .header("User-Agent", BROWSER_USER_AGENT)
}

fn image_url(image: ElementRef<'_>) -> Option<String> {
    ["data-original", "data-src", "data-cdn", "src"]
        .into_iter()
        .find_map(|name| attr(&image, name))
}

fn attr(element: &ElementRef<'_>, name: &str) -> Option<String> {
    element
        .value()
        .attr(name)
        .map(clean_text)
        .filter(|value| !value.is_empty() && !value.starts_with("data:"))
}

fn first_text(document: &Html, selector: &str) -> Result<Option<String>> {
    let selector = select(selector)?;
    Ok(document
        .select(&selector)
        .map(element_text)
        .find(|value| !value.is_empty()))
}

fn first_attr(document: &Html, selector: &str, name: &str) -> Result<Option<String>> {
    let selector = select(selector)?;
    Ok(document
        .select(&selector)
        .find_map(|element| attr(&element, name)))
}

fn has_page_link(document: &Html, page: u32) -> Result<bool> {
    let selector = select("a[href*='page=']")?;
    Ok(document.select(&selector).any(|anchor| {
        anchor
            .value()
            .attr("href")
            .and_then(|href| Url::parse(&absolute_url(href).ok()?).ok())
            .and_then(|url| {
                url.query_pairs()
                    .find(|(key, _)| key == "page")
                    .and_then(|(_, value)| value.parse::<u32>().ok())
            })
            == Some(page)
    }))
}

fn absolute_url(candidate: &str) -> Result<String> {
    if candidate.starts_with("//") {
        return Ok(format!("https:{candidate}"));
    }
    let base = Url::parse(BASE_URL).map_err(url_error)?;
    base.join(candidate)
        .map(|url| url.to_string())
        .map_err(url_error)
}

fn element_text(element: ElementRef<'_>) -> String {
    clean_text(&element.text().collect::<Vec<_>>().join(" "))
}

fn clean_text(input: &str) -> String {
    input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('\u{a0}', " ")
}

fn select(value: &str) -> Result<Selector> {
    Selector::parse(value)
        .map_err(|error| Error::new(format!("invalid selector {value:?}: {error}")))
}

fn url_error(error: impl ToString) -> Error {
    Error::new(format!("NekoRaw URL error: {}", error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    const CATALOG: &str = include_str!("../fixtures/catalog.html");
    const DETAILS: &str = include_str!("../fixtures/details.html");
    const CHAPTER: &str = include_str!("../fixtures/chapter.html");
    const MANIFEST: &str = include_str!("../manifest.json");
    const ICON: &[u8] = include_bytes!("../assets/icon.png");
    const ICON_SHA256: &str = "16e49143efada6f3a00ab54e882ce164562e7f595fe78778615044f8972fb35e";

    #[test]
    fn parses_current_catalog_markup_and_pagination() {
        let page = parse_catalog(&Html::parse_document(CATALOG), 1).expect("catalog parses");
        assert_eq!(page.entries.len(), 2);
        assert!(page.has_next_page);
        assert_eq!(page.entries[0].key, "wanpisu");
        assert_eq!(page.entries[0].title, "ワンピース");
        assert_eq!(
            page.entries[0]
                .cover
                .as_ref()
                .map(|cover| cover.url.as_str()),
            Some("https://admin.mangarawad.vip/storage/images/wanpisu/cover.jpg")
        );
        assert_eq!(
            page.entries[0].content_rating.as_deref(),
            Some(CONTENT_RATING)
        );
    }

    #[test]
    fn parses_current_details_and_chapters() {
        let document = Html::parse_document(DETAILS);
        let item =
            parse_details(&document, "wanpisu", &manga_url("wanpisu")).expect("details parse");
        assert_eq!(item.title, "ワンピース");
        assert_eq!(item.status, Some(json!("ongoing")));
        assert_eq!(item.tags, vec!["アクション", "ファンタジー"]);
        assert!(item.initialized);

        let chapters = parse_chapters(&document, "wanpisu").expect("chapters parse");
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].key, "chapter-1189");
        assert_eq!(chapters[0].chapter_number, Some(1189.0));
    }

    #[test]
    fn detail_status_ignores_unrelated_page_text() {
        let document = Html::parse_document(
            "<span>完結</span><div id='main-content'><span>連載中</span></div>",
        );
        assert_eq!(parse_detail_status(&document).unwrap(), "ongoing");
    }

    #[test]
    fn parses_page_images_with_referer_context() {
        let chapter_url = format!("{BASE_URL}/manga/wanpisu/chapter-1175");
        let pages = parse_pages(&Html::parse_document(CHAPTER), &chapter_url).expect("pages parse");
        assert_eq!(pages.len(), 3);
        let PageContent::Url { url, context } = &pages[1].content else {
            panic!("expected URL page");
        };
        assert_eq!(url, "https://iphotomg.com/wanpisu/1175/2.jpg");
        assert_eq!(
            context.as_ref().and_then(|headers| headers.get("Referer")),
            Some(&chapter_url)
        );
        assert_eq!(
            context
                .as_ref()
                .and_then(|headers| headers.get("User-Agent"))
                .map(String::as_str),
            Some(BROWSER_USER_AGENT)
        );
    }

    #[test]
    fn search_urls_and_legacy_deep_links_are_canonical() {
        let url = search_url(
            "one piece",
            2,
            &json!({"genre": "Action", "status": "0", "sort": "10"}),
            None,
        )
        .expect("URL builds");
        let parsed = Url::parse(&url).expect("URL parses");
        let query = parsed.query_pairs().collect::<BTreeMap<_, _>>();
        assert_eq!(
            query.get("keyword").map(|value| value.as_ref()),
            Some("one piece")
        );
        assert_eq!(
            query.get("genre").map(|value| value.as_ref()),
            Some("Action")
        );
        assert_eq!(query.get("status").map(|value| value.as_ref()), Some("0"));
        assert_eq!(query.get("sort").map(|value| value.as_ref()), Some("10"));
        assert_eq!(query.get("page").map(|value| value.as_ref()), Some("2"));

        let mut source = NekorawSource::default();
        let resolved = source
            .handle_url("https://nekoraw.blog/manga/wanpisu/chapter-1175")
            .expect("deep link parses")
            .expect("deep link resolves");
        assert_eq!(resolved.item.expect("item").url, Some(manga_url("wanpisu")));
        assert_eq!(resolved.chapter_key.as_deref(), Some("chapter-1175"));
    }

    #[test]
    fn exposes_every_original_search_filter() {
        let filters = filter_definitions();
        let FilterDefinition::Select { options, .. } = &filters[0] else {
            panic!("genre filter must be a select");
        };
        assert_eq!(options.len(), 27);
        assert_eq!(
            options.first().map(|option| option.value.as_str()),
            Some("")
        );
        assert_eq!(
            options.last().map(|option| option.value.as_str()),
            Some("Shoujo")
        );

        let FilterDefinition::Select { options, .. } = &filters[1] else {
            panic!("status filter must be a select");
        };
        assert_eq!(
            options
                .iter()
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>(),
            vec!["-1", "0", "1"]
        );

        let FilterDefinition::Select { options, .. } = &filters[2] else {
            panic!("sort filter must be a select");
        };
        assert_eq!(
            options
                .iter()
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>(),
            vec!["-1", "15", "10", "11", "12", "13", "20", "30"]
        );
    }

    #[test]
    fn manifest_and_icon_metadata_are_consistent() {
        let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest parses");
        assert_eq!(manifest["id"], "nekoraw");
        assert_eq!(manifest["sources"][0]["baseUrl"], BASE_URL);
        assert_eq!(manifest["sources"][0]["contentRating"], CONTENT_RATING);
        assert_eq!(manifest["assets"][0]["sha256"], ICON_SHA256);
        assert_eq!(format!("{:x}", Sha256::digest(ICON)), ICON_SHA256);
    }
}
