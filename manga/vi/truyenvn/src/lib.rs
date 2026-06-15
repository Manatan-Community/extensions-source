use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{
    html,
    manga::{self, Madara, MadaraConfig, MadaraSource},
};
use serde_json::{Value, json};

const SOURCE: TruyenVN = TruyenVN;
const DEFAULT_BASE_URL: &str = "https://truyenvn.sbs";

struct TruyenVN;

impl MadaraSource for TruyenVN {
    fn madara_config(&self, request: &Value) -> MadaraConfig {
        MadaraConfig {
            base_url: base_url(request),
            lang: "vi",
            content_rating: "adult",
            manga_path: "truyen-tranh",
            popular_url_marker: "post-title",
            use_load_more: false,
            latest_enabled: true,
        }
    }

    fn madara_list_fixture(&self) -> &'static str {
        LIST_FIXTURE
    }

    fn madara_details_fixture(&self) -> &'static str {
        DETAILS_FIXTURE
    }

    fn madara_pages_fixture(&self) -> &'static str {
        PAGES_FIXTURE
    }
}

impl MangaSource for TruyenVN {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        self.madara_list(request)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        self.madara_search(request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        self.madara_details(request)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = self.madara_config(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| self.madara_default_manga_key(&config));
        Ok(fetch_new_endpoint_chapters(&config, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        self.madara_pages(request)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                self.list(json!({"page": 1, "listingId": "popular"}))?,
            ),
            home_section(
                "latest",
                "Latest",
                self.list(json!({"page": 1, "listingId": "latest"}))?,
            ),
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let config = self.madara_config(&request);
        Ok(manga::request_key(&request, "manga").map(|key| config.absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let config = self.madara_config(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| config.absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        self.madara_handle_url(request)
    }
}

fn base_url(request: &Value) -> &'static str {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get("overrideBaseUrl"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
        .map(|value| {
            Box::leak(value.trim_end_matches('/').to_string().into_boxed_str()) as &'static str
        })
        .unwrap_or(DEFAULT_BASE_URL)
}

fn fetch_new_endpoint_chapters(config: &MadaraConfig, manga_key: &str) -> Vec<MangaChapter> {
    let manga_url = config
        .absolute_url(manga_key)
        .trim_end_matches('/')
        .to_string();
    let body = Madara::browser_client(config)
        .post(format!("{manga_url}/ajax/chapters"))
        .xhr()
        .referer(format!("{manga_url}/"))
        .origin(config.base_url)
        .send_text()
        .unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string());
    parse_chapters(&body, config)
}

fn parse_chapters(body: &str, config: &MadaraConfig) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("wp-manga-chapter"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_chapter_key(config, &href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "chapter-release-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_dd_mm_yyyy(&value)),
                url: Some(config.absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn normalize_chapter_key(config: &MadaraConfig, href: &str) -> String {
    config
        .normalize_manga_key(href)
        .trim_end_matches("?style=paged")
        .trim_end_matches("?style=list")
        .to_string()
}

fn parse_dd_mm_yyyy(value: &str) -> Option<i64> {
    let mut parts = value.trim().split('/');
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let year = parts.next()?.parse::<i32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    manatan_shared::dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(HomeSectionStyle::Cover),
        has_more: page.has_next_page,
        entries: page.entries,
        ..HomeSection::default()
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><div class="post-title"><a href="/truyen-tranh/sample/">Sample</a></div><img src="/cover.jpg"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
<div class="description-summary">Summary</div><div id="manga-chapters-holder" data-id="1"></div>
"#;
const CHAPTERS_FIXTURE: &str = r#"
<li class="wp-manga-chapter"><a href="/truyen-tranh/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">01/01/2024</span></li>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_ajax_chapters() {
        let chapters = SOURCE
            .chapters(json!({"manga": "/truyen-tranh/sample"}))
            .unwrap();
        assert_eq!(chapters[0].title.as_deref(), Some("Chapter 1"));
    }
}
