use manatan_extension::{
    AlternateCover, CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter,
    MangaPage, MangaPageImage, Paged, ProcessedImage, UrlResolveResult, Viewer,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, url};
use serde_json::{Value, json};

const BASE_URL: &str = "https://manga.example";
const SOURCE: ExampleManga = ExampleManga;

struct ExampleManga;

impl MangaSource for ExampleManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let html = if listing == "latest" {
            LATEST_FIXTURE
        } else {
            POPULAR_FIXTURE
        };
        Ok(Paged {
            entries: parse_listing(html),
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

    fn chapters(&self, _request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        Ok(parse_chapters(CHAPTERS_FIXTURE))
    }

    fn pages(&self, _request: Value) -> ExtensionResult<Vec<MangaPage>> {
        Ok(parse_pages(PAGES_FIXTURE))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: parse_listing(POPULAR_FIXTURE),
                has_more: true,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Compact),
                entries: parse_listing(LATEST_FIXTURE),
                has_more: true,
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.contains("/title/") {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(DETAILS_FIXTURE)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(None)
    }

    fn resolve_page_image(&self, request: Value) -> ExtensionResult<MangaPageImage> {
        let key = request
            .get("page")
            .and_then(|page| page.get("content"))
            .and_then(|content| content.get("lazy"))
            .and_then(|lazy| lazy.get("key"))
            .and_then(Value::as_str)
            .unwrap_or("page-1");
        Ok(MangaPageImage {
            url: url::join_url(BASE_URL, &format!("/images/{key}.jpg")),
            headers: manga::image_headers(BASE_URL),
            mime_type: Some("image/jpeg".to_string()),
            ..MangaPageImage::default()
        })
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        Ok(ProcessedImage {
            image_base64: manga::decrypt_fixture_image_base64(
                request
                    .get("imageBase64")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
            mime_type: request
                .get("mimeType")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            ..ProcessedImage::default()
        })
    }

    fn alternate_covers(&self, _request: Value) -> ExtensionResult<Vec<AlternateCover>> {
        Ok(vec![AlternateCover {
            url: url::join_url(BASE_URL, "/covers/iron-lantern-alt.jpg"),
            thumbnail: Some(url::join_url(BASE_URL, "/covers/iron-lantern-alt-thumb.jpg")),
            language: Some("en".to_string()),
            volume: Some("1".to_string()),
            description: Some("Volume cover".to_string()),
            headers: manga::image_headers(BASE_URL),
            ..AlternateCover::default()
        }])
    }
}

fn parse_listing(input: &str) -> Vec<CatalogItem> {
    input
        .split("<article")
        .skip(1)
        .filter_map(|chunk| {
            let key = html::attr_after(chunk, "data-key", "data-key")
                .or_else(|| html::attr(chunk, "data-key"))?;
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| key.clone());
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|path| url::join_url(BASE_URL, &path)),
                url: Some(url::join_url(BASE_URL, &format!("/title/{key}"))),
                tags: vec!["action".to_string(), "fantasy".to_string()],
                status: ItemStatus::Ongoing,
                viewer: Some(Viewer::RightToLeft),
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(input: &str) -> CatalogItem {
    CatalogItem {
        key: "iron-lantern".to_string(),
        title: html::text_between(input, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_else(|| "Iron Lantern".to_string()),
        alternate_titles: vec!["Lantern of Iron".to_string()],
        cover: html::attr_after(input, "<img", "src").map(|path| url::join_url(BASE_URL, &path)),
        url: Some(url::join_url(BASE_URL, "/title/iron-lantern")),
        authors: vec!["Example Author".to_string()],
        artists: vec!["Example Artist".to_string()],
        description: html::text_between(input, "<p", "</p>").map(|value| html::strip_tags(&value)),
        tags: vec!["action".to_string(), "fantasy".to_string()],
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        latest_update: dates::parse_fixture_date("2024-01-01"),
        status: ItemStatus::Ongoing,
        viewer: Some(Viewer::RightToLeft),
        alternate_covers: vec![AlternateCover {
            url: url::join_url(BASE_URL, "/covers/iron-lantern-alt.jpg"),
            ..AlternateCover::default()
        }],
        extra: [("deepLink".to_string(), json!(url::join_url(BASE_URL, "/title/iron-lantern")))]
            .into_iter()
            .collect(),
        ..CatalogItem::default()
    }
}

fn parse_chapters(input: &str) -> Vec<MangaChapter> {
    input
        .split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let key = html::attr(chunk, "data-key")?;
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)),
                chapter_number: key.rsplit('-').next().and_then(|value| value.parse().ok()),
                date_uploaded: dates::parse_fixture_date("2024-01-01"),
                scanlators: vec!["Example Scan Group".to_string()],
                language: Some("en".to_string()),
                page_count: Some(4),
                url: Some(url::join_url(BASE_URL, &format!("/title/iron-lantern/{key}"))),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(input: &str) -> Vec<MangaPage> {
    let mut pages = Vec::new();
    for chunk in input.split("<img").skip(1) {
        if let Some(key) = html::attr(chunk, "data-key") {
            pages.push(manga::lazy_page(&key, &url::join_url(BASE_URL, "/reader/iron-lantern/1")));
        }
    }
    pages.push(manga::archive_page(
        &url::join_url(BASE_URL, "/archives/iron-lantern-1.cbz"),
        "003.jpg",
    ));
    pages.push(manga::text_page("A text page can be used when the SDK host supports it."));
    pages
}

const POPULAR_FIXTURE: &str = r#"
<article data-key="iron-lantern"><img src="/covers/iron-lantern.jpg"><h3>Iron Lantern</h3></article>
<article data-key="paper-city"><img src="/covers/paper-city.jpg"><h3>Paper City</h3></article>
"#;

const LATEST_FIXTURE: &str = r#"
<article data-key="glass-road"><img src="/covers/glass-road.jpg"><h3>Glass Road</h3></article>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Iron Lantern</h1>
<img src="/covers/iron-lantern.jpg">
<p>A fixture manga used to demonstrate details parsing.</p>
"#;

const CHAPTERS_FIXTURE: &str = r#"
<li data-key="chapter-1"><a href="/title/iron-lantern/chapter-1">The First Light</a></li>
<li data-key="chapter-2"><a href="/title/iron-lantern/chapter-2">A Door Opens</a></li>
"#;

const PAGES_FIXTURE: &str = r#"
<img data-key="page-1" data-src="/images/page-1-token">
<img data-key="page-2" data-src="/images/page-2-token">
"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;
    use manatan_extension::PageContent;

    #[test]
    fn parses_details_fixture() {
        let item = parse_details(DETAILS_FIXTURE);
        assert_eq!(item.key, "iron-lantern");
        assert_eq!(item.alternate_covers.len(), 1);
    }

    #[test]
    fn parses_chapter_fixture() {
        let chapters = parse_chapters(CHAPTERS_FIXTURE);
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].chapter_number, Some(1.0));
    }

    #[test]
    fn parses_lazy_and_archive_pages() {
        let pages = parse_pages(PAGES_FIXTURE);
        assert!(matches!(pages[0].content, PageContent::Lazy { .. }));
        assert!(matches!(pages[2].content, PageContent::ArchiveEntry { .. }));
    }
}
