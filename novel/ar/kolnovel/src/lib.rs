use manatan_common::{absolute_url, attr, normalize_space, require, selector};
use manatan_sdk::{
    client::Client,
    html::{self, Html},
    model::{
        CatalogItem, FilterDefinition, ImageRequest, ImageRequestContext, NovelChapter,
        NovelContentBlock, NovelText, OptionItem, Paged, UrlResolveResult,
    },
    Error, NovelSource, Result,
};
use regex::Regex;
use serde_json::{json, Value};
use url::Url;

#[cfg(target_arch = "wasm32")]
const SOURCE_ID: &str = "kolnovel";
const BASE_URL: &str = "https://kolnovel.com";
// KolNovel regularly takes longer than the host's 45-second default to start
// returning HTML. Keep this below Manatan's two-minute source browse budget.
const HTTP_TIMEOUT_MS: u32 = 110_000;

pub struct KolNovelSource {
    client: Client,
}

impl Default for KolNovelSource {
    fn default() -> Self {
        Self {
            client: Client::browser().cookies_for(BASE_URL),
        }
    }
}

impl KolNovelSource {
    fn document(&self, url: &str) -> Result<(Html, String)> {
        let response = self
            .client
            .get(url)
            .timeout_ms(HTTP_TIMEOUT_MS)
            .send()?
            .error_for_status()?;
        let final_url = response.final_url().to_owned();
        Ok((html::document(response.text()?), final_url))
    }

    fn series_url(page: u32, order: &str, filters: &Value) -> Result<String> {
        let mut url = Url::parse(&format!("{BASE_URL}/series/"))
            .map_err(|error| Error::new(error.to_string()))?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("page", &page.max(1).to_string());
            query.append_pair(
                "order",
                filters
                    .get("order")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .unwrap_or(order),
            );
            if let Some(status) = filters
                .get("status")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                query.append_pair("status", status);
            }
            for genre in selected_filter_values(filters, "genres") {
                query.append_pair("genre[]", &genre);
            }
            for kind in selected_filter_values(filters, "types") {
                query.append_pair("type[]", &kind);
            }
        }
        Ok(url.to_string())
    }

    fn browse(&self, page: u32, order: &str, filters: &Value) -> Result<Paged<CatalogItem>> {
        let url = Self::series_url(page, order, filters)?;
        let (document, _) = self.document(&url)?;
        Self::parse_catalog(&document, page)
    }

    fn search_page(&self, query: &str, page: u32) -> Result<Paged<CatalogItem>> {
        let page = page.max(1);
        let mut url = Url::parse(&format!("{BASE_URL}/page/{page}/"))
            .map_err(|error| Error::new(error.to_string()))?;
        url.query_pairs_mut().append_pair("s", query.trim());
        let (document, _) = self.document(url.as_str())?;
        Self::parse_catalog(&document, page)
    }

    fn parse_catalog(document: &Html, page: u32) -> Result<Paged<CatalogItem>> {
        let cards = selector(".listupd article.maindet, article.maindet")?;
        let links = selector("h2 a[href*=\"/series/\"]")?;
        let covers = selector("img.ts-post-image")?;
        let descriptions = selector(".contexcerpt")?;
        let genres = selector(".mdgenre a")?;
        let mut items = Vec::new();
        for card in document.select(&cards) {
            let Some(link) = card.select(&links).next() else {
                continue;
            };
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let title = normalize_space(&html::text(link));
            if title.is_empty() {
                continue;
            }
            let page_url = absolute_url(BASE_URL, &href)?;
            let tags = card
                .select(&genres)
                .map(html::text)
                .map(|value| normalize_space(value.trim_start_matches('#')))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            let mut item = CatalogItem::new(page_url.clone(), title);
            item.url = Some(page_url.clone());
            item.description = card
                .select(&descriptions)
                .next()
                .map(html::text)
                .map(|value| normalize_space(&value))
                .filter(|value| !value.is_empty());
            item.tags = tags;
            item.cover = card
                .select(&covers)
                .next()
                .and_then(|node| {
                    ["data-src", "data-lazy-src", "src"]
                        .iter()
                        .find_map(|name| attr(node, name))
                })
                .map(|cover| absolute_url(BASE_URL, &cover))
                .transpose()?
                .map(|cover| image(&cover, &page_url));
            item.language = Some("ar".into());
            item.content_rating = Some(content_rating(&item.tags).into());
            items.push(item);
        }
        let has_next = has_next_page(document, page)?;
        Ok(Paged::new(items, has_next))
    }

    fn item_url(item: &CatalogItem) -> Result<String> {
        let candidate = item.url.as_deref().unwrap_or(&item.key);
        let mut url = Url::parse(&absolute_url(BASE_URL, candidate)?)
            .map_err(|error| Error::new(error.to_string()))?;
        url.set_query(None);
        url.set_fragment(None);
        if !url.path().starts_with("/series/") {
            return Err(Error::new("KolNovel item URL is not a series page"));
        }
        if !url.path().ends_with('/') {
            let path = format!("{}/", url.path());
            url.set_path(&path);
        }
        Ok(url.to_string())
    }

    fn parse_details(document: &Html, page_url: &str) -> Result<CatalogItem> {
        let title = first_text(document, "h1.entry-title")?
            .ok_or_else(|| Error::new("KolNovel series has no title"))?;
        let tags = texts(document, ".sertogenre a")?;
        let mut item = CatalogItem::new(page_url, title);
        item.url = Some(page_url.into());
        item.cover = first_attr(document, ".sertothumb img", "src")?
            .map(|cover| absolute_url(BASE_URL, &cover))
            .transpose()?
            .map(|cover| image(&cover, page_url));
        item.description = first_text(document, ".sersys.entry-content > p")?;
        item.authors = texts(document, ".sertoauth a[href*=\"/writer/\"]")?;
        item.tags = tags;
        item.status =
            first_text(document, ".sertostat span")?.map(|status| json!(normalize_status(&status)));
        item.language = Some("ar".into());
        item.content_rating = Some(content_rating(&item.tags).into());
        item.initialized = true;
        Ok(item)
    }

    fn parse_chapters(document: &Html) -> Result<Vec<NovelChapter>> {
        let rows = selector(".eplister li")?;
        let links = selector("a[href]")?;
        let numbers = selector(".epl-num")?;
        let titles = selector(".epl-title")?;
        let prices = selector(".epl-price")?;
        let number_re =
            Regex::new(r"(\d+(?:\.\d+)?)").map_err(|error| Error::new(error.to_string()))?;
        let mut chapters = Vec::new();
        for row in document.select(&rows) {
            let Some(link) = row.select(&links).next() else {
                continue;
            };
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let chapter_url = absolute_url(BASE_URL, &href)?;
            let number_label = row
                .select(&numbers)
                .next()
                .map(html::text)
                .map(|value| normalize_space(&value))
                .unwrap_or_default();
            let chapter_number = number_re
                .captures(&number_label)
                .and_then(|captures| captures.get(1))
                .and_then(|value| value.as_str().parse::<f32>().ok());
            let raw_title = row
                .select(&titles)
                .next()
                .map(html::text)
                .map(|value| normalize_space(&value))
                .filter(|value| !value.is_empty());
            let price = row
                .select(&prices)
                .next()
                .map(html::text)
                .map(|value| normalize_space(&value))
                .unwrap_or_default();
            let locked = number_label.contains('🔒')
                || (!price.is_empty()
                    && !matches!(
                        price.to_ascii_lowercase().as_str(),
                        "free" | "gratuit" | "livre"
                    )
                    && price != "مجاني");
            let base_title = raw_title
                .or_else(|| (!number_label.is_empty()).then_some(number_label.clone()))
                .or_else(|| chapter_number.map(|number| format!("الفصل {number}")));
            let title = base_title.map(|title| {
                if locked && !title.starts_with('🔒') {
                    format!("🔒 {title}")
                } else {
                    title
                }
            });
            chapters.push(NovelChapter {
                key: chapter_url.clone(),
                title,
                chapter_number,
                url: Some(chapter_url),
                language: Some("ar".into()),
                ..NovelChapter::default()
            });
        }
        chapters.reverse();
        for (index, chapter) in chapters.iter_mut().enumerate() {
            chapter.source_order = Some(index as i32);
        }
        require(
            (!chapters.is_empty()).then_some(()),
            "KolNovel series has no chapters",
        )?;
        Ok(chapters)
    }

    fn parse_text(document: &Html, chapter_url: &str) -> Result<NovelText> {
        let raw = first_inner_html(document, ".epcontent")?.ok_or_else(|| {
            Error::new("KolNovel chapter is unavailable. Open Web View, sign in, then retry.")
        })?;
        let rendered = sanitize_chapter_html(document, &raw)?;
        let readable = normalize_space(&html::text(html::fragment(&rendered).root_element()));
        let login_only = [
            "أنت غير مسجل الدخول لمشاهدة المحتوى",
            "إنشاء حساب جديد والإشتراك",
            "تسجيل الدخول لمشاهدة المحتوى",
        ]
        .iter()
        .any(|message| readable.contains(message));
        require(
            (!readable.is_empty() && !login_only).then_some(()),
            "KolNovel requires an active account or subscription for this chapter. Open Web View, sign in, then retry.",
        )?;
        Ok(NovelText {
            html: Some(rendered.clone()),
            title: first_text(document, "h1.entry-title, .epheader h1")?,
            base_url: Some(chapter_url.into()),
            image_context: Some(ImageRequestContext {
                headers: [("Referer".into(), chapter_url.into())]
                    .into_iter()
                    .collect(),
                cookie_url: Some(BASE_URL.into()),
            }),
            blocks: vec![NovelContentBlock::Text {
                text: rendered,
                html: true,
            }],
            ..NovelText::default()
        })
    }
}

impl NovelSource for KolNovelSource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse(page, "popular", &json!({}))
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse(page, "update", &json!({}))
    }

    fn listing(&mut self, listing: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        match listing {
            "popular" => self.browse(page, "popular", filters),
            "latest" => self.browse(page, "update", filters),
            _ => Err(Error::new(format!("unknown KolNovel listing {listing:?}"))),
        }
    }

    fn search(&mut self, query: &str, page: u32, _filters: &Value) -> Result<Paged<CatalogItem>> {
        self.search_page(query, page)
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
        let url = absolute_url(BASE_URL, chapter.url.as_deref().unwrap_or(&chapter.key))?;
        let (document, final_url) = self.document(&url)?;
        Self::parse_text(&document, &final_url)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![
            select_filter("order", "الترتيب", ORDER),
            select_filter("status", "الحالة", STATUS),
            FilterDefinition::Group {
                id: "genres".into(),
                name: "التصنيفات".into(),
                filters: checkbox_filters(GENRES),
            },
            FilterDefinition::Group {
                id: "types".into(),
                name: "النوع".into(),
                filters: checkbox_filters(TYPES),
            },
        ])
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let url = Url::parse(candidate).map_err(|error| Error::new(error.to_string()))?;
        if !matches!(url.host_str(), Some("kolnovel.com" | "www.kolnovel.com")) {
            return Ok(None);
        }
        let series_re = Regex::new(r"^/series/([^/]+)/?").unwrap();
        if let Some(captures) = series_re.captures(url.path()) {
            let item_url = format!("{BASE_URL}/series/{}/", &captures[1]);
            let mut item = CatalogItem::new(item_url.clone(), "");
            item.url = Some(item_url);
            item.language = Some("ar".into());
            return Ok(Some(UrlResolveResult {
                item: Some(item),
                ..UrlResolveResult::default()
            }));
        }
        let chapter_re = Regex::new(r"^/shaag24(.+)z435ggye-(\d+)/?").unwrap();
        let Some(captures) = chapter_re.captures(url.path()) else {
            return Ok(None);
        };
        let item_url = format!("{BASE_URL}/series/{}/", &captures[1]);
        let mut item = CatalogItem::new(item_url.clone(), "");
        item.url = Some(item_url);
        item.language = Some("ar".into());
        let chapter = NovelChapter {
            key: candidate.into(),
            url: Some(candidate.into()),
            language: Some("ar".into()),
            ..NovelChapter::default()
        };
        Ok(Some(UrlResolveResult {
            item: Some(item),
            novel_chapter: Some(chapter),
            ..UrlResolveResult::default()
        }))
    }
}

fn first_text(document: &Html, query: &str) -> Result<Option<String>> {
    let query = selector(query)?;
    Ok(document
        .select(&query)
        .next()
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

fn first_attr(document: &Html, query: &str, name: &str) -> Result<Option<String>> {
    let query = selector(query)?;
    Ok(document
        .select(&query)
        .find_map(|element| attr(element, name)))
}

fn first_inner_html(document: &Html, query: &str) -> Result<Option<String>> {
    let query = selector(query)?;
    Ok(document.select(&query).next().map(|node| node.inner_html()))
}

fn has_next_page(document: &Html, page: u32) -> Result<bool> {
    let links = selector("a[href]")?;
    for link in document.select(&links) {
        let Some(href) = attr(link, "href") else {
            continue;
        };
        let Ok(url) = Url::parse(&absolute_url(BASE_URL, &href)?) else {
            continue;
        };
        if url
            .query_pairs()
            .find_map(|(key, value)| (key == "page").then(|| value.into_owned()))
            .and_then(|value| value.parse::<u32>().ok())
            == Some(page + 1)
            || url.path() == format!("/page/{}/", page + 1)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn sanitize_chapter_html(document: &Html, value: &str) -> Result<String> {
    let mut rendered = value.to_owned();
    let styles = selector("article > style")?;
    let hidden_classes = Regex::new(r"\.([A-Za-z0-9_-]+)\s*[,\\{]")
        .map_err(|error| Error::new(error.to_string()))?;
    for style in document.select(&styles) {
        let css = style.inner_html();
        for captures in hidden_classes.captures_iter(&css) {
            let class = regex::escape(&captures[1]);
            let paragraph = Regex::new(&format!(
                r#"(?is)<p\b[^>]*class\s*=\s*["'][^"']*\b{class}\b[^"']*["'][^>]*>.*?</p\s*>"#
            ))
            .map_err(|error| Error::new(error.to_string()))?;
            rendered = paragraph.replace_all(&rendered, "").into_owned();
        }
    }
    let unsafe_elements = Regex::new(
        r#"(?is)<(?:script|style|iframe|object|embed|form|noscript)\b[^>]*>.*?</(?:script|style|iframe|object|embed|form|noscript)\s*>|<(?:script|style|iframe|object|embed|form|noscript)\b[^>]*/?>"#,
    )
    .map_err(|error| Error::new(error.to_string()))?;
    rendered = unsafe_elements.replace_all(&rendered, "").into_owned();
    let code_blocks = Regex::new(
        r#"(?is)<(?:div|span|p)\b[^>]*class\s*=\s*["'][^"']*\bcode-block\b[^"']*["'][^>]*>.*?</(?:div|span|p)\s*>"#,
    )
    .map_err(|error| Error::new(error.to_string()))?;
    rendered = code_blocks.replace_all(&rendered, "").into_owned();
    let event = Regex::new(r#"(?i)\s+on[a-z]+\s*=\s*(?:"[^"]*"|'[^']*')"#)
        .map_err(|error| Error::new(error.to_string()))?;
    Ok(event.replace_all(&rendered, "").into_owned())
}

fn image(url: &str, referer: &str) -> ImageRequest {
    ImageRequest::get(url)
        .header("Referer", referer)
        .cookies_for(BASE_URL)
}

fn selected_filter_values(filters: &Value, key: &str) -> Vec<String> {
    match filters.get(key) {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect(),
        Some(Value::Object(values)) => values
            .iter()
            .filter_map(|(value, enabled)| {
                enabled
                    .as_bool()
                    .unwrap_or(false)
                    .then_some(value.to_owned())
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn normalize_status(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "ongoing" | "مستمرة" => "ongoing",
        "completed" | "complete" | "مكتملة" => "completed",
        "hiatus" | "متوقفة" => "hiatus",
        _ => "unknown",
    }
}

fn content_rating(tags: &[String]) -> &'static str {
    if tags.iter().any(|tag| {
        matches!(
            tag.trim().to_ascii_lowercase().as_str(),
            "adult" | "mature" | "ecchi" | "smut" | "explicit"
        ) || matches!(tag.trim(), "ناضج" | "إيتشي" | "للبالغين")
    }) {
        "adult"
    } else {
        "suggestive"
    }
}

fn select_filter(id: &str, name: &str, values: &[(&str, &str)]) -> FilterDefinition {
    FilterDefinition::Select {
        id: id.into(),
        name: name.into(),
        options: values
            .iter()
            .map(|(label, value)| OptionItem {
                label: (*label).into(),
                value: (*value).into(),
            })
            .collect(),
        default_index: 0,
    }
}

fn checkbox_filters(values: &[(&str, &str)]) -> Vec<FilterDefinition> {
    values
        .iter()
        .map(|(label, value)| FilterDefinition::CheckBox {
            id: (*value).into(),
            name: (*label).into(),
            default: false,
        })
        .collect()
}

const ORDER: &[(&str, &str)] = &[
    ("الافتراضي", ""),
    ("آخر التحديثات", "update"),
    ("الأحدث إضافة", "latest"),
    ("الرائجة", "popular"),
    ("التقييم", "rating"),
    ("العنوان A-Z", "title"),
    ("العنوان Z-A", "titlereverse"),
];
const STATUS: &[(&str, &str)] = &[
    ("الكل", ""),
    ("مستمرة", "ongoing"),
    ("متوقفة", "hiatus"),
    ("مكتملة", "completed"),
];
const GENRES: &[(&str, &str)] = &[
    ("أكشن", "action"),
    ("مغامرة", "adventure"),
    ("الخيال العلمي", "sci-fi"),
    ("فنون القتال", "martial-arts"),
    ("فانتازيا", "fantasy"),
    ("رومانسي", "romantic"),
    ("كوميدي", "comedy"),
    ("دراما", "drama"),
    ("غموض", "mysteries"),
    ("رعب", "horror"),
    ("حريم", "harem"),
    ("إيسيكاي", "isekai"),
    ("حياة مدرسية", "school-life"),
    ("شريحة من الحياة", "slice-of-life"),
    ("خارق للطبيعة", "supernatural"),
    ("مأساوي", "tragedy"),
];
const TYPES: &[(&str, &str)] = &[
    ("رواية شبكية", "web-novel"),
    ("رواية خفيفة", "light-novel"),
    ("قصة قصيرة", "one-shot"),
    ("صينية", "chinese"),
    ("كورية", "korean"),
    ("يابانية", "japanese"),
    ("عربية", "arabic"),
    ("إنجليزية", "english"),
];

#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel(SOURCE_ID, KolNovelSource::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_catalog_cover_tags_and_pagination() {
        let document = html::document(include_str!("../tests/fixtures/catalog.html"));
        let page = KolNovelSource::parse_catalog(&document, 1).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].title, "رواية الاختبار");
        assert_eq!(page.entries[0].language.as_deref(), Some("ar"));
        assert_eq!(page.entries[0].tags, vec!["أكشن", "الخيال العلمي"]);
        assert!(page.entries[0]
            .cover
            .as_ref()
            .unwrap()
            .url
            .ends_with("/cover.jpg"));
        assert!(page.has_next_page);
    }

    #[test]
    fn parses_details_and_oldest_first_chapters() {
        let document = html::document(include_str!("../tests/fixtures/details.html"));
        let item = KolNovelSource::parse_details(&document, "https://kolnovel.com/series/fixture/")
            .unwrap();
        assert_eq!(item.title, "رواية الاختبار");
        assert_eq!(item.authors, vec!["كاتب الاختبار"]);
        assert_eq!(item.status, Some(json!("ongoing")));
        assert!(item.initialized);

        let chapters = KolNovelSource::parse_chapters(&document).unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].chapter_number, Some(1.0));
        assert_eq!(chapters[1].chapter_number, Some(2.0));
        assert!(chapters[1].title.as_deref().unwrap().starts_with('🔒'));
        assert_eq!(chapters[0].source_order, Some(0));
    }

    #[test]
    fn removes_obfuscated_and_unsafe_chapter_content() {
        let document = html::document(include_str!("../tests/fixtures/chapter.html"));
        let text =
            KolNovelSource::parse_text(&document, "https://kolnovel.com/chapter-fixture/").unwrap();
        let rendered = text.html.unwrap();
        assert!(rendered.contains("هذا هو النص الحقيقي"));
        assert!(!rendered.contains("نص مزيف"));
        assert!(!rendered.contains("إعلان مخفي"));
        assert!(!rendered.contains("<script"));
        assert!(!rendered.contains("onclick"));
    }

    #[test]
    fn fails_closed_for_login_only_chapter() {
        let document = html::document(include_str!("../tests/fixtures/locked.html"));
        let error =
            KolNovelSource::parse_text(&document, "https://kolnovel.com/locked/").unwrap_err();
        assert!(error.to_string().contains("active account or subscription"));
    }

    #[test]
    fn serializes_filters_and_resolves_urls() {
        let url = KolNovelSource::series_url(
            2,
            "popular",
            &json!({
                "order": "rating",
                "status": "ongoing",
                "genres": {"action": true, "horror": false},
                "types": ["korean"]
            }),
        )
        .unwrap();
        let parsed = Url::parse(&url).unwrap();
        let pairs = parsed.query_pairs().collect::<Vec<_>>();
        assert!(pairs
            .iter()
            .any(|pair| pair == &("page".into(), "2".into())));
        assert!(pairs
            .iter()
            .any(|pair| pair == &("genre[]".into(), "action".into())));
        assert!(pairs
            .iter()
            .any(|pair| pair == &("type[]".into(), "korean".into())));

        let mut source = KolNovelSource::default();
        let resolved = source
            .handle_url("https://kolnovel.com/shaag24fixturez435ggye-123/")
            .unwrap()
            .unwrap();
        assert_eq!(
            resolved.item.unwrap().url.as_deref(),
            Some("https://kolnovel.com/series/fixture/")
        );
        assert!(resolved.novel_chapter.is_some());
    }

    #[test]
    #[ignore = "requires KOLNOVEL_LIVE_CATALOG and KOLNOVEL_LIVE_DETAILS HTML files"]
    fn parses_captured_live_pages() {
        let catalog_path = std::env::var("KOLNOVEL_LIVE_CATALOG").expect("KOLNOVEL_LIVE_CATALOG");
        let details_path = std::env::var("KOLNOVEL_LIVE_DETAILS").expect("KOLNOVEL_LIVE_DETAILS");
        let catalog = html::document(
            std::fs::read_to_string(catalog_path)
                .expect("read live catalog")
                .as_str(),
        );
        let page = KolNovelSource::parse_catalog(&catalog, 1).expect("parse live catalog");
        assert!(!page.entries.is_empty());
        assert!(page
            .entries
            .iter()
            .all(|item| item.language.as_deref() == Some("ar")));
        assert!(page.entries.iter().any(|item| item.cover.is_some()));

        let details = html::document(
            std::fs::read_to_string(details_path)
                .expect("read live details")
                .as_str(),
        );
        let item =
            KolNovelSource::parse_details(&details, "https://kolnovel.com/series/48hours-a-day/")
                .expect("parse live details");
        assert_eq!(item.title, "48 ساعة باليوم");
        assert!(!item.authors.is_empty());
        assert!(item.cover.is_some());
        let chapters = KolNovelSource::parse_chapters(&details).expect("parse live chapters");
        assert!(chapters.len() > 100);
        assert_eq!(
            chapters.first().and_then(|chapter| chapter.source_order),
            Some(0)
        );
    }

    #[test]
    #[ignore = "requires KOLNOVEL_LIVE_CHAPTER HTML file"]
    fn parses_captured_live_chapter() {
        let chapter_path = std::env::var("KOLNOVEL_LIVE_CHAPTER").expect("KOLNOVEL_LIVE_CHAPTER");
        let chapter = html::document(
            std::fs::read_to_string(chapter_path)
                .expect("read live chapter")
                .as_str(),
        );
        let text = KolNovelSource::parse_text(
            &chapter,
            "https://kolnovel.com/shaag2448hours-a-dayz435ggye-100093/",
        )
        .expect("parse live chapter");
        let rendered = text.html.expect("chapter HTML");
        assert!(normalize_space(&html::text(html::fragment(&rendered).root_element())).len() > 100);
    }
}
