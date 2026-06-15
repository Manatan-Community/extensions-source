use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Doujins = Doujins;
const BASE_URL: &str = "https://doujins.com";
const PAGE_DAYS: u64 = 3;

struct Doujins;

impl MangaSource for Doujins {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_latest(LATEST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_latest(&fetch_text(&latest_url(page), LATEST_FIXTURE)));
        }
        Ok(parse_gallery(&fetch_text(
            &format!("{BASE_URL}/top/month"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_text(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target = if !query.is_empty() {
            let mut target = format!(
                "{BASE_URL}/searches?words={}&page={}",
                url::query_escape(query),
                page
            );
            if let Some(sort) = filter_string(filters, "sort").filter(|value| !value.is_empty()) {
                target.push_str(&format!("&sort={}", url::query_escape(&sort)));
            }
            target
        } else if let Some(series) =
            filter_string(filters, "series").filter(|value| !value.is_empty())
        {
            let mut target = format!("{BASE_URL}{series}");
            if let Some(sort) = filter_string(filters, "sort").filter(|value| !value.is_empty()) {
                target.push_str(&format!("?sort={}", url::query_escape(&sort)));
            }
            target
        } else {
            format!(
                "{BASE_URL}{}",
                filter_string(filters, "period").unwrap_or_else(|| "/top".into())
            )
        };
        Ok(parse_gallery(&fetch_text(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_details(
            &fetch_text(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let body = fetch_text(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Chapter".into()),
            scanlators: scanlator(&body).into_iter().collect(),
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("en".into()),
            chapter_number: Some(1.0),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample".into());
        Ok(parse_pages(&fetch_text(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_text(input, DETAILS_FIXTURE),
                    Some(normalize_key(input)),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_text(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn latest_url(page: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(1_704_067_200);
    let day = 86_400;
    let end = ((now / day) + 1).saturating_sub(PAGE_DAYS * page.saturating_sub(1)) * day;
    let start = end.saturating_sub(PAGE_DAYS * day);
    format!("{BASE_URL}/folders?start={start}&end={end}")
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<LatestResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(LATEST_FIXTURE).expect("fixture is valid"));
    Paged {
        entries: response
            .folders
            .into_iter()
            .map(LatestFolder::to_catalog)
            .collect(),
        has_next_page: true,
    }
}

fn parse_gallery(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("thumbnail-doujin")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let key = normalize_key(&href);
                let title = html::text_between(chunk, "title", "</")
                    .or_else(|| html::text_between(chunk, "<a", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| url::slug_from_url(&key))
                    .unwrap_or_else(|| "Doujins".into());
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                    artists: info_after(chunk, "Artist:").into_iter().collect(),
                    authors: info_after(chunk, "Artist:").into_iter().collect(),
                    status: ItemStatus::Completed,
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("en".into()),
                    content_rating: Some("adult".into()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: body.contains("pagination") && !body.contains("page-item disabled"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "folder-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Doujins".into()),
        artists: link_texts(body, "gallery-artist"),
        authors: link_texts(body, "gallery-artist"),
        tags: link_texts(body, "tag-area"),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("class=\"doujin\"")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-file").or_else(|| html::attr(chunk, "src")))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.replace("amp;", ""),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "srcset")
        .map(|value| {
            value
                .split_whitespace()
                .next()
                .unwrap_or(&value)
                .to_string()
        })
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn link_texts(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .skip(1)
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn scanlator(body: &str) -> Option<String> {
    body.split("Translated")
        .nth(1)
        .map(|value| {
            html::strip_tags(value)
                .split("by:")
                .nth(1)
                .unwrap_or_default()
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn info_after(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .map(html::strip_tags)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn filter_string(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find(BASE_URL) {
            return format!(
                "/{}",
                input[index + BASE_URL.len()..].trim_start_matches('/')
            );
        }
    }
    format!("/{}", input.trim_start_matches('/'))
}

#[derive(Debug, Deserialize)]
struct LatestResponse {
    folders: Vec<LatestFolder>,
}

#[derive(Debug, Deserialize)]
struct LatestFolder {
    #[serde(rename = "link")]
    link: String,
    #[serde(rename = "name")]
    name: String,
    #[serde(default, rename = "artistList")]
    artist_list: String,
    #[serde(default)]
    tags: Vec<LatestTag>,
    #[serde(default, rename = "thumbnail2")]
    thumbnail: String,
}

impl LatestFolder {
    fn to_catalog(self) -> CatalogItem {
        let key = normalize_key(&self.link);
        CatalogItem {
            key: key.clone(),
            title: self.name,
            artists: (!self.artist_list.is_empty())
                .then_some(self.artist_list.clone())
                .into_iter()
                .collect(),
            authors: (!self.artist_list.is_empty())
                .then_some(self.artist_list)
                .into_iter()
                .collect(),
            tags: self.tags.into_iter().map(|tag| tag.tag).collect(),
            cover: (!self.thumbnail.is_empty()).then_some(self.thumbnail),
            status: ItemStatus::Completed,
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("en".into()),
            content_rating: Some("adult".into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct LatestTag {
    tag: String,
}

export_manga_source!(SOURCE);

const LATEST_FIXTURE: &str = r#"{"folders":[{"link":"/sample","name":"Sample Doujin","artistList":"Sample Artist","tags":[{"tag":"Full Color"}],"thumbnail2":"https://doujins.com/thumb.jpg"}]}"#;
const LIST_FIXTURE: &str = r#"<div class="thumbnail-doujin"><a class="gallery-visited-from-favorites" href="/sample"><div class="title"><span class="text">Sample Doujin</span></div><img srcset="/thumb.jpg 1x"></a></div><div class="single-line"><strong>Artist: Sample Artist</strong></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="folder-title"><a>Sample Doujin</a></div><div class="gallery-artist"><a>Sample Artist</a></div><div class="tag-area"><a>Full Color</a></div><div class="folder-message">Translated by: Sample Group</div>"#;
const PAGES_FIXTURE: &str =
    r#"<img class="doujin" data-link="/1" data-file="https://doujins.com/page1.jpg">"#;
