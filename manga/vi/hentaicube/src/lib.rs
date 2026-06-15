use manatan_extension::{
    MangaPage, PageContent, Paged, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::manga::{self, Madara, MadaraConfig, MadaraSource};
use serde_json::Value;

const SOURCE: HentaiCube = HentaiCube;
const DEFAULT_BASE_URL: &str = "https://2tencb.pro";

struct HentaiCube;

impl MadaraSource for HentaiCube {
    fn madara_config(&self, request: &Value) -> MadaraConfig {
        MadaraConfig {
            base_url: base_url(request),
            lang: "vi",
            content_rating: "adult",
            manga_path: "read",
            popular_url_marker: "post-title",
            use_load_more: true,
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

    fn madara_listing_order(&self, request: &Value) -> &'static str {
        match request
            .get("filters")
            .and_then(|filters| filters.get("sort"))
            .and_then(Value::as_str)
        {
            Some("latest") => "latest",
            Some("new-manga") => "new-manga",
            Some("alphabet") => "alphabet",
            Some("rating") => "rating",
            Some("trending") => "trending",
            _ if request.get("listingId").and_then(Value::as_str) == Some("latest") => "latest",
            _ => "views",
        }
    }
}

impl MangaSource for HentaiCube {
    fn list(&self, request: Value) -> ExtensionResult<Paged<manatan_extension::CatalogItem>> {
        MadaraSource::madara_list(self, request)
    }

    fn search(&self, mut request: Value) -> ExtensionResult<Paged<manatan_extension::CatalogItem>> {
        let config = self.madara_config(&request);
        if let Some(query) = request.get("query").and_then(Value::as_str) {
            if let Some(slug) = query
                .strip_prefix("id:")
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                request["query"] = Value::String(format!("{}/read/{slug}/", config.base_url));
            }
        }
        MadaraSource::madara_search(self, request)
    }

    fn details(&self, request: Value) -> ExtensionResult<manatan_extension::CatalogItem> {
        MadaraSource::madara_details(self, request)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<manatan_extension::MangaChapter>> {
        MadaraSource::madara_chapters(self, request)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = self.madara_config(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| self.madara_default_chapter_key(&config));
        let chapter_url = config.absolute_url(&key);
        let body = Madara::fetch_document_or_fixture(&config, &chapter_url, PAGES_FIXTURE);
        if body.contains("manga-secure-reader") {
            let payload = Madara::browser_client(&config)
                .get(format!(
                    "{}/wp-json/manga-reader/v1/images",
                    config.base_url
                ))
                .referer(chapter_url)
                .xhr()
                .send_text()
                .unwrap_or_else(|_| SECURE_READER_FIXTURE.to_string());
            return Ok(secure_reader_pages(&payload, config.base_url));
        }
        Ok(Madara::parse_pages(&body, &config))
    }

    fn handle_url(
        &self,
        request: Value,
    ) -> ExtensionResult<Option<manatan_extension::UrlResolveResult>> {
        MadaraSource::madara_handle_url(self, request)
    }
}

fn base_url(request: &Value) -> &'static str {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get("overrideBaseUrl"))
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
        .map(|value| {
            Box::leak(value.trim_end_matches('/').to_string().into_boxed_str()) as &'static str
        })
        .unwrap_or(DEFAULT_BASE_URL)
}

fn secure_reader_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("images").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

const LIST_FIXTURE: &str = r#"<div class="page-item-detail"><div class="post-title"><a href="/read/sample/">Sample</a></div><img src="/cover.jpg"></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="post-title"><h1>Sample</h1></div><div class="summary_image"><img src="/cover.jpg"></div><div class="description-summary">Summary</div><li class="wp-manga-chapter"><a href="/read/sample/chapter-1/">Chapter 1</a></li>"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;
const SECURE_READER_FIXTURE: &str = r#"{"images":["https://2tencb.pro/page1.jpg"]}"#;

export_manga_source!(SOURCE);
