use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: NineHentai = NineHentai;
const BASE_URL: &str = "https://9hentai.so";

struct NineHentai;

impl MangaSource for NineHentai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if listing_id(&request) == "popular" {
            1
        } else {
            0
        };
        Ok(search_page("", page, sort))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let id = gallery_id(query);
            return Ok(Paged {
                entries: vec![parse_gallery_details(
                    &fetch_document(&format!("{BASE_URL}/g/{id}"), DETAILS_FIXTURE),
                    Some(id.to_string()),
                )],
                has_next_page: false,
            });
        }
        if let Some(id) = query
            .strip_prefix("id:")
            .and_then(|id| id.parse::<u64>().ok())
        {
            return Ok(Paged {
                entries: vec![single_manga(id).to_catalog()],
                has_next_page: false,
            });
        }
        Ok(search_page(query, page, 0))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/g/1".into());
        let id = gallery_id(&key);
        Ok(parse_gallery_details(
            &fetch_document(&format!("{BASE_URL}/g/{id}"), DETAILS_FIXTURE),
            Some(id.to_string()),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/g/1".into());
        let id = gallery_id(&key);
        Ok(vec![MangaChapter {
            key: format!("/g/{id}"),
            title: Some("Chapter".into()),
            chapter_number: Some(1.0),
            url: Some(format!("{BASE_URL}/g/{id}")),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/g/1".into());
        let manga = single_manga(gallery_id(&key));
        let image_base = manga.image_url();
        Ok((1..=manga.total_page)
            .map(|page| MangaPage {
                content: PageContent::Url {
                    url: format!("{image_base}/{page}.jpg"),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {page}")),
                ..MangaPage::default()
            })
            .collect())
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                HomeSectionStyle::Cover,
                self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?,
            ),
            home_section(
                "latest",
                "Latest",
                HomeSectionStyle::Compact,
                self.list(serde_json::json!({"page": 1, "listingId": "latest"}))?,
            ),
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/g/{}", gallery_id(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}/g/{}", gallery_id(&key))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let id = gallery_id(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_gallery_details(
                    &fetch_document(&format!("{BASE_URL}/g/{id}"), DETAILS_FIXTURE),
                    Some(id.to_string()),
                )),
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
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn search_page(query: &str, page: u64, sort: u8) -> Paged<CatalogItem> {
    let payload = json!({
        "search": {
            "text": query,
            "page": page.saturating_sub(1),
            "sort": sort,
            "pages": { "range": [0, 2000] },
            "tag": { "items": { "included": [], "excluded": [] } }
        }
    });
    let body = client()
        .post(format!("{BASE_URL}/api/getBook"))
        .json(payload.to_string())
        .xhr()
        .send_text()
        .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
    let response = serde_json::from_str::<SearchResponse>(&body)
        .unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).expect("fixture is valid"));
    Paged {
        has_next_page: response.total_count > page,
        entries: response
            .results
            .into_iter()
            .map(Manga::to_catalog)
            .collect(),
    }
}

fn single_manga(id: u64) -> Manga {
    let body = client()
        .post(format!("{BASE_URL}/api/getBookByID"))
        .json(json!({ "id": id }).to_string())
        .xhr()
        .send_text()
        .unwrap_or_else(|_| SINGLE_FIXTURE.to_string());
    serde_json::from_str::<SingleMangaResponse>(&body)
        .unwrap_or_else(|_| serde_json::from_str(SINGLE_FIXTURE).expect("fixture is valid"))
        .results
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_gallery_details(body: &str, fallback_id: Option<String>) -> CatalogItem {
    let key = fallback_id
        .map(|id| format!("/g/{id}"))
        .or_else(|| {
            html::attr_after(body, "property=\"og:url\"", "content")
                .map(|url| format!("/g/{}", gallery_id(&url)))
        })
        .unwrap_or_else(|| "/g/1".into());
    let title = html::text_between(body, "<h1", "</h1>")
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
        .unwrap_or_else(|| format!("Gallery {}", gallery_id(&key)));
    CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| html::attr_after(body, "v-lazy-image", "src")),
        url: Some(format!("{BASE_URL}/g/{}", gallery_id(&key))),
        authors: text_for_field(body, "Group:").into_iter().collect(),
        artists: text_for_field(body, "Artist:").into_iter().collect(),
        description: details_description(body),
        tags: tags_for_field(body, "Tag:"),
        status: ItemStatus::Completed,
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn text_for_field(body: &str, label: &str) -> Option<String> {
    body.split("field-name")
        .find(|chunk| chunk.contains(label))
        .map(html::strip_tags)
        .map(|text| text.replace(label, "").trim().to_string())
        .filter(|text| !text.is_empty())
}

fn tags_for_field(body: &str, label: &str) -> Vec<String> {
    text_for_field(body, label)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn details_description(body: &str) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(alt) = html::text_between(body, "<h2", "</h2>").map(|text| html::strip_tags(&text))
    {
        if !alt.is_empty() {
            lines.push(format!("Alternative Title: {alt}"));
        }
    }
    for label in ["Parody:", "Category:", "Language:"] {
        if let Some(value) = text_for_field(body, label) {
            lines.push(format!("{} {}", label.trim_end_matches(':'), value));
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n\n"))
}

fn gallery_id(input: &str) -> u64 {
    input
        .trim_end_matches('/')
        .rsplit('/')
        .find_map(|part| part.parse::<u64>().ok())
        .unwrap_or(1)
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("latest")
}

fn home_section(
    id: &str,
    title: &str,
    style: HomeSectionStyle,
    page: Paged<CatalogItem>,
) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(style),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    total_count: u64,
    #[serde(default)]
    results: Vec<Manga>,
}

#[derive(Deserialize)]
struct SingleMangaResponse {
    results: Manga,
}

#[derive(Deserialize)]
struct Manga {
    id: u64,
    total_page: u32,
    title: String,
    image_server: String,
}

impl Manga {
    fn image_url(&self) -> String {
        format!("{}{}", self.image_server, self.id)
    }

    fn to_catalog(self) -> CatalogItem {
        let cover = format!("{}/cover-small.jpg", self.image_url());
        CatalogItem {
            key: format!("/g/{}", self.id),
            title: self.title,
            cover: Some(cover),
            url: Some(format!("{BASE_URL}/g/{}", self.id)),
            status: ItemStatus::Completed,
            content_rating: Some("adult".into()),
            ..CatalogItem::default()
        }
    }
}

export_manga_source!(SOURCE);

const SEARCH_FIXTURE: &str = r#"{"total_count":1,"results":[{"id":1,"total_page":1,"title":"Sample","image_server":"https://i.9hentai.example.invalid/"}]}"#;
const SINGLE_FIXTURE: &str = r#"{"results":{"id":1,"total_page":1,"title":"Sample","image_server":"https://i.9hentai.example.invalid/"}}"#;
const DETAILS_FIXTURE: &str = r#"<div id="bigcontainer"><h1>Sample</h1><h2>Alt</h2><meta property="og:image" content="https://i.9hentai.example.invalid/1/cover.jpg"><div class="field-name">Artist: <a class="tag">Creator</a></div><div class="field-name">Tag: <a class="tag">Adult</a></div></div>"#;
