use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{dates, html, novel, url};
use serde_json::{Value, json};

const BASE_URL: &str = "https://novel.example";
const SOURCE: ExampleNovel = ExampleNovel;

struct ExampleNovel;

impl NovelSource for ExampleNovel {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let fixture = if listing == "latest" {
            LATEST_FIXTURE
        } else {
            POPULAR_FIXTURE
        };
        Ok(Paged {
            entries: parse_listing(fixture),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![parse_details(DETAILS_FIXTURE)],
                has_next_page: false,
            });
        }
        let mut entries = parse_listing(POPULAR_FIXTURE);
        if !query.is_empty() {
            entries.retain(|item| item.title.to_lowercase().contains(&query.to_lowercase()));
        }
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, _request: Value) -> ExtensionResult<CatalogItem> {
        Ok(parse_details(DETAILS_FIXTURE))
    }

    fn chapters(&self, _request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        Ok(parse_chapters(CHAPTERS_FIXTURE).entries)
    }

    fn text(&self, _request: Value) -> ExtensionResult<NovelText> {
        Ok(parse_text(TEXT_FIXTURE))
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1) as u32;
        let mut parsed = parse_chapters(CHAPTERS_FIXTURE);
        parsed.next_page = parsed.has_next_page.then_some(page + 1);
        Ok(parsed)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: parse_listing(POPULAR_FIXTURE),
            has_more: true,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.contains("/novel/") {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(DETAILS_FIXTURE)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(None)
    }
}

fn parse_listing(input: &str) -> Vec<CatalogItem> {
    input
        .split("<article")
        .skip(1)
        .filter_map(|chunk| {
            let key = html::attr(chunk, "data-key")?;
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| key.clone());
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|path| url::join_url(BASE_URL, &path)),
                url: Some(url::join_url(BASE_URL, &format!("/novel/{key}"))),
                tags: vec!["fantasy".to_string()],
                language: Some("en".to_string()),
                status: ItemStatus::Ongoing,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(input: &str) -> CatalogItem {
    CatalogItem {
        key: "glass-library".to_string(),
        title: html::text_between(input, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_else(|| "Glass Library".to_string()),
        alternate_titles: vec!["Library of Glass".to_string()],
        cover: html::attr_after(input, "<img", "src").map(|path| url::join_url(BASE_URL, &path)),
        url: Some(url::join_url(BASE_URL, "/novel/glass-library")),
        authors: vec!["Example Writer".to_string()],
        description: html::text_between(input, "<p", "</p>").map(|value| html::strip_tags(&value)),
        tags: vec!["fantasy".to_string(), "mystery".to_string()],
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        latest_update: dates::parse_fixture_date("2024-01-01"),
        status: ItemStatus::Ongoing,
        extra: [("api".to_string(), parse_api_fixture(API_FIXTURE))]
            .into_iter()
            .collect(),
        ..CatalogItem::default()
    }
}

fn parse_chapters(input: &str) -> NovelChapterPage {
    let entries = input
        .split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let key = html::attr(chunk, "data-key")?;
            Some(NovelChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)),
                chapter_number: key.rsplit('-').next().and_then(|value| value.parse().ok()),
                date_uploaded: dates::parse_fixture_date("2024-01-01"),
                url: Some(url::join_url(BASE_URL, &format!("/novel/glass-library/{key}"))),
                language: Some("en".to_string()),
                word_count: Some(2400),
                ..NovelChapter::default()
            })
        })
        .collect();
    NovelChapterPage {
        entries,
        has_next_page: true,
        next_page: Some(2),
        section: Some("Volume 1".to_string()),
        ..NovelChapterPage::default()
    }
}

fn parse_text(input: &str) -> NovelText {
    let normalized = novel::normalize_reader_html(input);
    NovelText {
        title: Some("A Borrowed Key".to_string()),
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(BASE_URL.to_string()),
        css: Some("img { max-width: 100%; } body { line-height: 1.7; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        previous_chapter_key: None,
        next_chapter_key: Some("chapter-2".to_string()),
        extra: [(
            "binaryFixtureBytes".to_string(),
            json!(novel::decode_fixture_base64("fixture").len()),
        )]
        .into_iter()
        .collect(),
        ..NovelText::default()
    }
}

fn parse_api_fixture(input: &str) -> Value {
    serde_json::from_str(input).unwrap_or(Value::Null)
}

const POPULAR_FIXTURE: &str = r#"
<article data-key="glass-library"><img src="/covers/glass-library.jpg"><h3>Glass Library</h3></article>
<article data-key="map-of-rain"><img src="/covers/map-of-rain.jpg"><h3>Map of Rain</h3></article>
"#;

const LATEST_FIXTURE: &str = r#"
<article data-key="paper-moon"><img src="/covers/paper-moon.jpg"><h3>Paper Moon</h3></article>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Glass Library</h1>
<img src="/covers/glass-library.jpg">
<p>A fixture novel used to demonstrate details parsing.</p>
"#;

const CHAPTERS_FIXTURE: &str = r#"
<li data-key="chapter-1"><a href="/novel/glass-library/chapter-1">A Borrowed Key</a></li>
<li data-key="chapter-2"><a href="/novel/glass-library/chapter-2">Stacks at Midnight</a></li>
"#;

const TEXT_FIXTURE: &str = r#"
<section class="reader">
<h1>A Borrowed Key</h1>
<p>The library woke before the city did.</p>
<img data-src="/images/chapter-1-map.png">
</section>
"#;

const API_FIXTURE: &str = r#"{"rating":4.8,"serverTokenRequired":true}"#;

export_novel_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_details_fixture() {
        let item = parse_details(DETAILS_FIXTURE);
        assert_eq!(item.key, "glass-library");
        assert_eq!(item.authors[0], "Example Writer");
    }

    #[test]
    fn parses_chapter_page_fixture() {
        let page = parse_chapters(CHAPTERS_FIXTURE);
        assert_eq!(page.entries.len(), 2);
        assert!(page.has_next_page);
    }

    #[test]
    fn parses_text_fixture() {
        let text = parse_text(TEXT_FIXTURE);
        assert!(text.html.as_deref().unwrap_or_default().contains("src="));
        assert!(text.text.as_deref().unwrap_or_default().contains("library woke"));
    }
}
