use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: SpyFakku = SpyFakku;
const BASE_URL: &str = "https://hentalk.pw";
const API_URL: &str = "https://hentalk.pw/api";

struct SpyFakku;

impl MangaSource for SpyFakku {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "created_at"
        } else {
            "released_at"
        };
        let response: HentaiLib = fetch_json_or_fixture(
            &format!("{API_URL}/library?sort={sort}&page={page}"),
            LIB_FIXTURE,
        );
        Ok(response.into_page())
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = gallery_id(query) {
            let hentai: Hentai =
                fetch_json_or_fixture(&format!("{API_URL}/g/{id}"), HENTAI_FIXTURE);
            return Ok(Paged {
                entries: vec![hentai.to_item(false)],
                has_next_page: false,
            });
        }
        let mut target = format!(
            "{API_URL}/library?page={page}&sort=relevance&q={}",
            url::query_escape(query)
        );
        if query.is_empty() {
            target = format!("{API_URL}/library?page={page}&sort=released_at");
        }
        let response: HentaiLib = fetch_json_or_fixture(&target, LIB_FIXTURE);
        Ok(response.into_page())
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/g/1".to_string());
        let id = gallery_id(&key).unwrap_or(1);
        let details: ShortHentai =
            fetch_json_or_fixture(&format!("{API_URL}/g/{id}"), DETAILS_FIXTURE);
        Ok(details.to_item(id, key_hash(&key), key_pages(&key), true))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/g/1".to_string());
        let id = gallery_id(&key).unwrap_or(1);
        let details: ShortHentai =
            fetch_json_or_fixture(&format!("{API_URL}/g/{id}"), DETAILS_FIXTURE);
        let hash = key_hash(&key)
            .or_else(|| Some(details.hash.clone()))
            .unwrap();
        let pages = key_pages(&key).unwrap_or(details.pages);
        Ok(vec![MangaChapter {
            key: format!("/g/{id}?{pages}&hash={hash}"),
            title: Some("Chapter".to_string()),
            page_count: Some(pages as u32),
            url: Some(format!("{BASE_URL}/g/{id}")),
            language: Some("en".to_string()),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/g/1?1&hash=sample".to_string());
        let id = gallery_id(&key).unwrap_or(1);
        let (hash, pages) = match (key_hash(&key), key_pages(&key)) {
            (Some(hash), Some(pages)) => (hash, pages),
            _ => {
                let details: ShortHentai =
                    fetch_json_or_fixture(&format!("{API_URL}/g/{id}"), DETAILS_FIXTURE);
                (details.hash, details.pages)
            }
        };
        Ok((1..=pages)
            .map(|page| MangaPage {
                content: PageContent::Url {
                    url: format!("{BASE_URL}/image/{hash}/{page}"),
                    context: None,
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {page}")),
                ..MangaPage::default()
            })
            .collect())
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = gallery_id(input) {
            let details: ShortHentai =
                fetch_json_or_fixture(&format!("{API_URL}/g/{id}"), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(details.to_item(id, key_hash(input), key_pages(input), true)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.to_string(),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture<T: for<'de> Deserialize<'de>>(target: &str, fixture: &str) -> T {
    let text = client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string());
    serde_json::from_str(&text).unwrap_or_else(|_| serde_json::from_str(fixture).unwrap())
}

fn gallery_id(input: &str) -> Option<u64> {
    let marker = "/g/";
    let start = input
        .find(marker)
        .map(|index| index + marker.len())
        .unwrap_or(0);
    input[start..]
        .split(['/', '?', '#', '&'])
        .next()
        .and_then(|value| value.parse().ok())
}

fn key_pages(input: &str) -> Option<u64> {
    input
        .split('?')
        .nth(1)
        .and_then(|query| query.split('&').next())
        .and_then(|value| value.parse().ok())
}

fn key_hash(input: &str) -> Option<String> {
    input.split("hash=").nth(1).and_then(|value| {
        value
            .split(['&', '#'])
            .next()
            .filter(|hash| !hash.is_empty())
            .map(ToString::to_string)
    })
}

#[derive(Debug, Deserialize)]
struct HentaiLib {
    archives: Vec<Hentai>,
    page: u64,
    limit: u64,
    total: u64,
}

impl HentaiLib {
    fn into_page(self) -> Paged<CatalogItem> {
        Paged {
            entries: self
                .archives
                .into_iter()
                .map(|hentai| hentai.to_item(false))
                .collect(),
            has_next_page: self.page * self.limit < self.total,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Hentai {
    id: u64,
    hash: String,
    title: String,
    thumbnail: u64,
    pages: u64,
    tags: Option<Vec<Name>>,
}

impl Hentai {
    fn to_item(&self, initialized: bool) -> CatalogItem {
        let grouped = TagGroups::new(self.tags.as_deref().unwrap_or(&[]));
        CatalogItem {
            key: format!("/g/{}?{}&hash={}", self.id, self.pages, self.hash),
            title: self.title.clone(),
            cover: Some(format!(
                "{BASE_URL}/image/{}/{}?type=cover",
                self.hash, self.thumbnail
            )),
            url: Some(format!("{BASE_URL}/g/{}", self.id)),
            authors: grouped.circles(),
            artists: grouped.artists(),
            tags: grouped.tags(),
            status: ItemStatus::Completed,
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ShortHentai {
    hash: String,
    thumbnail: u64,
    description: Option<String>,
    #[serde(default)]
    tags: Option<Vec<Name>>,
    #[allow(dead_code)]
    size: u64,
    pages: u64,
}

impl ShortHentai {
    fn to_item(
        &self,
        id: u64,
        hash_from_key: Option<String>,
        pages_from_key: Option<u64>,
        initialized: bool,
    ) -> CatalogItem {
        let hash = hash_from_key.unwrap_or_else(|| self.hash.clone());
        let pages = pages_from_key.unwrap_or(self.pages);
        let grouped = TagGroups::new(self.tags.as_deref().unwrap_or(&[]));
        CatalogItem {
            key: format!("/g/{id}?{pages}&hash={hash}"),
            title: format!("Gallery {id}"),
            cover: Some(format!(
                "{BASE_URL}/image/{hash}/{}?type=cover",
                self.thumbnail
            )),
            url: Some(format!("{BASE_URL}/g/{id}")),
            authors: grouped.circles_or_artists(),
            artists: grouped.artists(),
            description: self.description.clone(),
            tags: grouped.tags(),
            status: ItemStatus::Completed,
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct Name {
    namespace: String,
    name: String,
}

struct TagGroups<'a> {
    tags: &'a [Name],
}

impl<'a> TagGroups<'a> {
    fn new(tags: &'a [Name]) -> Self {
        Self { tags }
    }

    fn by_namespace(&self, namespace: &str) -> Vec<String> {
        self.tags
            .iter()
            .filter(|tag| tag.namespace == namespace)
            .map(|tag| tag.name.clone())
            .collect()
    }

    fn circles(&self) -> Vec<String> {
        self.by_namespace("circle")
    }

    fn artists(&self) -> Vec<String> {
        self.by_namespace("artist")
    }

    fn circles_or_artists(&self) -> Vec<String> {
        let circles = self.circles();
        if circles.is_empty() {
            self.artists()
        } else {
            circles
        }
    }

    fn tags(&self) -> Vec<String> {
        self.by_namespace("tag")
    }
}

export_manga_source!(SOURCE);

const LIB_FIXTURE: &str = r#"{"archives":[{"id":1,"hash":"samplehash","title":"Sample Gallery","thumbnail":1,"pages":1,"tags":[{"namespace":"tag","name":"sample"}]}],"page":1,"limit":24,"total":1}"#;
const HENTAI_FIXTURE: &str = r#"{"id":1,"hash":"samplehash","title":"Sample Gallery","thumbnail":1,"pages":1,"tags":[{"namespace":"tag","name":"sample"}]}"#;
const DETAILS_FIXTURE: &str = r#"{"hash":"samplehash","thumbnail":1,"description":"Sample gallery","tags":[{"namespace":"tag","name":"sample"}],"size":1,"pages":1}"#;
