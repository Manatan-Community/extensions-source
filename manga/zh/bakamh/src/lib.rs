use manatan_extension::{MangaChapter, abi::ExtensionResult, export_manga_source};
use manatan_shared::{
    html,
    manga::{Madara, MadaraConfig, MadaraSource},
    sdk::{CatalogItem, MangaPage, Paged, UrlResolveResult, source::MangaSource},
};
use serde_json::Value;

const SOURCE: Bakamh = Bakamh;
const BASE_URL: &str = "https://www.bakamh.com";
const CONFIG: MadaraConfig = MadaraConfig {
    base_url: BASE_URL,
    lang: "zh",
    content_rating: "adult",
    manga_path: "manga",
    popular_url_marker: "<a",
    use_load_more: false,
    latest_enabled: true,
};

struct Bakamh;

impl MadaraSource for Bakamh {
    fn madara_config(&self, _request: &Value) -> MadaraConfig {
        CONFIG
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

impl MangaSource for Bakamh {
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
        let key = manatan_shared::manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/manga/sample".to_string());
        let body = Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        let manga_url = CONFIG.absolute_url(&key).to_ascii_lowercase();
        let mut chapters = body
            .split("<a")
            .skip(1)
            .filter(|chunk| {
                chunk.contains("storage-chapter-url")
                    || chunk.contains("onclick")
                    || chunk.to_ascii_lowercase().contains(&manga_url)
                    || chunk.contains("/manga/")
            })
            .filter_map(|chunk| parse_chapter(chunk, &manga_url))
            .collect::<Vec<_>>();
        if chapters.is_empty() {
            chapters = Madara::parse_chapters(&body, &key, &CONFIG);
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        self.madara_pages(request)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        self.madara_handle_url(request)
    }
}

fn parse_chapter(chunk: &str, manga_url: &str) -> Option<MangaChapter> {
    let href = html::attr(chunk, "storage-chapter-url")
        .or_else(|| html::attr(chunk, "href"))
        .or_else(|| first_attr_value_containing(chunk, manga_url))?;
    let key = CONFIG.normalize_manga_key(&href);
    let title = html::text_between(&format!("<a{chunk}"), "<a", "</a>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Chapter".to_string());
    Some(MangaChapter {
        key: key.clone(),
        title: Some(title),
        url: Some(CONFIG.absolute_url(&key)),
        ..MangaChapter::default()
    })
}

fn first_attr_value_containing(chunk: &str, needle: &str) -> Option<String> {
    for part in chunk.split_whitespace() {
        let (_, raw) = part.split_once('=')?;
        let value = raw.trim_matches(['"', '\'', '>']);
        let lower = value.to_ascii_lowercase();
        if lower.starts_with(needle) && !lower.ends_with("#comment") {
            return Some(value.to_string());
        }
    }
    None
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><a href="https://www.bakamh.com/manga/sample/" title="Sample"><img src="https://www.bakamh.com/cover.jpg"></a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="post-title">Sample</h1><div class="summary_image"><img src="https://www.bakamh.com/cover.jpg"></div><div class="description-summary">Sample description.</div>
<ul><li><a storage-chapter-url="https://www.bakamh.com/manga/sample/chapter-1/">第 1 话</a></li></ul>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="reading-content"><img class="wp-manga-chapter-img" src="https://www.bakamh.com/page-1.jpg"></div>
"#;
