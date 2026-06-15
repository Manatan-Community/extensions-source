use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage,
    MangaPageImage, PageContent, Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: TerraHistoricus = TerraHistoricus;
const BASE_URL: &str = "https://comic.hypergryph.com";
const TOPICS: [&str; 2] = ["terra-historicus", "talos-ii-historicus"];

struct TerraHistoricus;

impl MangaSource for TerraHistoricus {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let topic = topic(page);
        let path = if listing(&request) == "latest" {
            format!("/api/recentUpdate?topicKey={topic}")
        } else {
            format!("/api/comic?topicKey={topic}")
        };
        if listing(&request) == "latest" {
            let updates: THResult<Vec<THRecentUpdate>> = get_json(&path)?;
            Ok(Paged {
                entries: updates.data.into_iter().map(THRecentUpdate::into_item).collect(),
                has_next_page: page < TOPICS.len() as u64,
            })
        } else {
            let comics: THResult<Vec<THComic>> = get_json(&path)?;
            Ok(Paged {
                entries: comics.data.into_iter().map(THComic::into_item).collect(),
                has_next_page: page < TOPICS.len() as u64,
            })
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if let Some(key) = key_from_url(&query) {
            return Ok(Paged {
                entries: vec![fetch_details(&key)?],
                has_next_page: false,
            });
        }
        let page = self.list(request)?;
        if query.is_empty() {
            return Ok(page);
        }
        Ok(Paged {
            has_next_page: page.has_next_page,
            entries: page
                .entries
                .into_iter()
                .filter(|item| item.title.contains(&query))
                .collect(),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/api/comic/1".to_string());
        fetch_details(&key)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/api/comic/1".to_string());
        let comic: THResult<THComic> = get_json(&key)?;
        Ok(comic.data.into_chapters())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/api/comic/1/episode/1".to_string());
        let episode: THResult<THEpisode> = get_json(&key)?;
        let page_count = episode.data.page_infos.unwrap_or_default().len();
        Ok((1..=page_count)
            .map(|page_num| {
                let page_key = format!("{key}/page?pageNum={page_num}");
                MangaPage {
                    content: PageContent::Lazy {
                        key: page_key.clone(),
                        url: Some(absolute(&page_key)),
                        page_url: Some(absolute(&key)),
                        context: None,
                    },
                    description: Some(format!("Page {page_num}")),
                    ..MangaPage::default()
                }
            })
            .collect())
    }

    fn resolve_page_image(&self, request: Value) -> ExtensionResult<MangaPageImage> {
        let key = request
            .get("page")
            .and_then(|page| page.get("content"))
            .and_then(|content| content.get("lazy"))
            .and_then(|lazy| lazy.get("key"))
            .and_then(Value::as_str)
            .unwrap_or("/api/comic/1/episode/1/page?pageNum=1");
        let page: THResult<THPage> = get_json(key)?;
        Ok(MangaPageImage {
            url: page.data.url,
            headers: manga::image_headers(BASE_URL),
            ..MangaPageImage::default()
        })
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute(&key).replace("/api", "")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute(&key).replace("/api", "")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&key)?),
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

fn get_json<T: for<'de> Deserialize<'de>>(path: &str) -> ExtensionResult<T> {
    let body = client().get(absolute(path)).xhr().send_text()?;
    serde_json::from_str(&body).map_err(|error| manatan_extension::abi::ExtensionError {
        message: error.to_string(),
    })
}

fn listing(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing_id"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn topic(page: u64) -> &'static str {
    TOPICS
        .get(page.saturating_sub(1) as usize)
        .copied()
        .unwrap_or(TOPICS[0])
}

fn absolute(path: &str) -> String {
    url::join_url(BASE_URL, path)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| input.trim_start_matches(BASE_URL).to_string())
        .filter(|key| key.contains("/comic/"))
        .map(|key| {
            if key.starts_with("/api/") {
                key
            } else {
                format!("/api{key}")
            }
        })
}

fn fetch_details(key: &str) -> ExtensionResult<CatalogItem> {
    let comic: THResult<THComic> = get_json(key)?;
    Ok(comic.data.into_item())
}

#[derive(Deserialize)]
struct THResult<T> {
    data: T,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct THComic {
    cid: String,
    #[serde(default)]
    r#type: u8,
    cover: String,
    title: String,
    #[serde(default)]
    subtitle: String,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(default)]
    keywords: Vec<String>,
    introduction: Option<String>,
    episodes: Option<Vec<THEpisode>>,
    update_time: Option<i64>,
}

impl THComic {
    fn into_item(self) -> CatalogItem {
        let mut tags = match self.r#type {
            2 => vec!["相簿".to_string()],
            3 => vec!["四格".to_string()],
            _ => Vec::new(),
        };
        tags.extend(self.keywords.into_iter());
        let description = if self.subtitle.is_empty() {
            self.introduction
        } else {
            Some(format!(
                "「{}」\n{}",
                self.subtitle,
                self.introduction.unwrap_or_default()
            ))
        };
        CatalogItem {
            key: format!("/api/comic/{}", self.cid),
            title: self.title,
            cover: Some(self.cover),
            authors: self.authors.clone(),
            artists: self.authors,
            tags,
            status: ItemStatus::Unknown,
            description,
            url: Some(format!("{BASE_URL}/comic/{}", self.cid)),
            language: Some("zh".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: self.episodes.is_some(),
            ..CatalogItem::default()
        }
    }

    fn into_chapters(self) -> Vec<MangaChapter> {
        let mut chapters = self
            .episodes
            .unwrap_or_default()
            .into_iter()
            .map(|episode| {
                let cid = episode.cid.unwrap_or_default();
                let title = match episode.r#type {
                    1 => episode
                        .short_title
                        .filter(|value| !value.is_empty())
                        .map(|short| format!("{short} {}", episode.title))
                        .unwrap_or(episode.title),
                    2 => format!("番外 {}", episode.title),
                    3 => format!("贺图 {}", episode.title),
                    4 => format!("公告 {}", episode.title),
                    _ => episode.title,
                };
                MangaChapter {
                    key: format!("/api/comic/{}/episode/{cid}", self.cid),
                    title: Some(title),
                    url: Some(format!("{BASE_URL}/comic/{}/episode/{cid}", self.cid)),
                    ..MangaChapter::default()
                }
            })
            .collect::<Vec<_>>();
        if let Some(first) = chapters.first_mut() {
            first.date_uploaded = self.update_time;
        }
        chapters
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct THRecentUpdate {
    cover_url: String,
    comic_cid: String,
    title: String,
}

impl THRecentUpdate {
    fn into_item(self) -> CatalogItem {
        CatalogItem {
            key: format!("/api/comic/{}", self.comic_cid),
            title: self.title,
            cover: Some(self.cover_url),
            url: Some(format!("{BASE_URL}/comic/{}", self.comic_cid)),
            language: Some("zh".to_string()),
            content_rating: Some("safe".to_string()),
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct THEpisode {
    cid: Option<String>,
    #[serde(default)]
    r#type: u8,
    short_title: Option<String>,
    title: String,
    page_infos: Option<Vec<THPageInfo>>,
}

#[derive(Deserialize)]
struct THPageInfo {
    #[allow(dead_code)]
    double_page: bool,
}

#[derive(Deserialize)]
struct THPage {
    url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_public_urls_to_api_keys() {
        assert_eq!(
            key_from_url("https://comic.hypergryph.com/comic/abc").unwrap(),
            "/api/comic/abc"
        );
    }
}

export_manga_source!(SOURCE);
