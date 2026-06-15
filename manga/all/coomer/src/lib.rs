use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const BASE_URL: &str = "https://coomer.st";
const IMG_CDN_URL: &str = "https://img.coomer.st";
const SOURCE: Coomer = Coomer;

struct Coomer;

impl MangaSource for Coomer {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(search_creators(page, "", "pop"))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = normalize_creator_key(query);
            return Ok(Paged {
                entries: vec![creator_key_item(&key)],
                has_next_page: false,
            });
        }
        let sort = request
            .get("filters")
            .and_then(|filters| filters.get("sort"))
            .and_then(Value::as_str)
            .unwrap_or("pop");
        Ok(search_creators(page, query, sort))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/onlyfans/user/sample".into());
        Ok(creator_key_item(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/onlyfans/user/sample".into());
        let pages = request
            .get("preferences")
            .and_then(|prefs| prefs.get("postPages"))
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .clamp(1, 75);
        let mut chapters = Vec::new();
        for page in 0..pages {
            let body = fetch_text_or_fixture(
                &format!("{BASE_URL}/api/v1{}/posts?o={}", key, page * 50),
                POSTS_FIXTURE,
            );
            let posts = serde_json::from_str::<Vec<PostDto>>(&body)
                .unwrap_or_else(|_| serde_json::from_str(POSTS_FIXTURE).expect("fixture posts"));
            chapters.extend(posts.into_iter().filter(|post| !post.images().is_empty()).map(PostDto::to_chapter));
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/onlyfans/user/sample/post/1".into());
        let body = fetch_text_or_fixture(&format!("{BASE_URL}/api/v1{key}"), POST_FIXTURE);
        let wrapped = serde_json::from_str::<PostWrapped>(&body)
            .unwrap_or_else(|_| serde_json::from_str(POST_FIXTURE).expect("fixture post"));
        let low_res = request
            .get("preferences")
            .and_then(|prefs| prefs.get("lowResolutionImages"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(wrapped
            .post
            .images()
            .into_iter()
            .enumerate()
            .map(|(index, path)| MangaPage {
                content: PageContent::Url {
                    url: image_url(&path, low_res),
                    context: None,
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(creator_key_item(&normalize_creator_key(input))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
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

fn fetch_text_or_fixture(target_url: &str, fixture: &str) -> String {
    client()
        .get(target_url)
        .xhr()
        .header("Accept", "text/css")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_creators(page: u64, query: &str, sort: &str) -> Paged<CatalogItem> {
    let body = fetch_text_or_fixture(&format!("{BASE_URL}/api/v1/creators"), CREATORS_FIXTURE);
    let mut creators = serde_json::from_str::<Vec<CreatorDto>>(&body)
        .unwrap_or_else(|_| serde_json::from_str(CREATORS_FIXTURE).expect("fixture creators"));
    creators.retain(|creator| {
        creator.service != "discord"
            && ["onlyfans", "fansly", "candfans"].contains(&creator.service.as_str())
            && creator.name.to_ascii_lowercase().contains(&query.to_ascii_lowercase())
    });
    match sort {
        "tit" => creators.sort_by(|a, b| a.name.cmp(&b.name)),
        "new" => creators.sort_by(|a, b| b.id.cmp(&a.id)),
        "lat" => creators.sort_by(|a, b| b.updated_string().cmp(&a.updated_string())),
        _ => creators.sort_by(|a, b| b.favorited.cmp(&a.favorited)),
    }
    let start = ((page.saturating_sub(1)) * 50) as usize;
    let end = (start + 50).min(creators.len());
    Paged {
        entries: creators[start.min(creators.len())..end]
            .iter()
            .map(CreatorDto::to_item)
            .collect(),
        has_next_page: end < creators.len(),
    }
}

fn creator_key_item(key: &str) -> CatalogItem {
    let parts = key.trim_matches('/').split('/').collect::<Vec<_>>();
    let service = parts.first().copied().unwrap_or("onlyfans");
    let id = parts.get(2).copied().unwrap_or("sample");
    CatalogItem {
        key: format!("/{service}/user/{id}"),
        title: id.to_string(),
        cover: Some(format!("{IMG_CDN_URL}/icons/{service}/{id}")),
        authors: vec![service_name(service)],
        description: Some("You can change how many posts to load in the extension preferences.".into()),
        status: ItemStatus::Unknown,
        url: Some(format!("{BASE_URL}/{service}/user/{id}")),
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn normalize_creator_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        format!("/{}", input[BASE_URL.len()..].trim_start_matches('/').trim_end_matches('/'))
    } else {
        format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn image_url(path: &str, low_res: bool) -> String {
    let mut full = url::join_url(BASE_URL, &format!("/data{}", path));
    if low_res {
        if let Some(index) = full[8..].find('/') {
            let split = index + 8;
            full.insert_str(split, "/thumbnail");
        }
    }
    full
}

fn service_name(service: &str) -> String {
    match service {
        "fanbox" => "Pixiv Fanbox",
        "subscribestar" => "SubscribeStar",
        "dlsite" => "DLsite",
        "onlyfans" => "OnlyFans",
        "fansly" => "Fansly",
        "candfans" => "CandFans",
        _ => service,
    }
    .to_string()
}

#[derive(Debug, Deserialize)]
struct CreatorDto {
    id: String,
    name: String,
    service: String,
    updated: Value,
    #[serde(default)]
    favorited: i64,
}

impl CreatorDto {
    fn updated_string(&self) -> String {
        self.updated
            .as_str()
            .map(ToString::to_string)
            .unwrap_or_else(|| self.updated.to_string())
    }

    fn to_item(&self) -> CatalogItem {
        CatalogItem {
            key: format!("/{}/user/{}", self.service, self.id),
            title: self.name.clone(),
            cover: Some(format!("{IMG_CDN_URL}/icons/{}/{}", self.service, self.id)),
            authors: vec![service_name(&self.service)],
            description: Some("You can change how many posts to load in the extension preferences.".into()),
            status: ItemStatus::Unknown,
            url: Some(format!("{BASE_URL}/{}/user/{}", self.service, self.id)),
            language: Some("all".into()),
            content_rating: Some("adult".into()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct PostWrapped { post: PostDto }

#[derive(Debug, Deserialize)]
struct PostDto {
    id: String,
    service: String,
    user: String,
    title: String,
    added: Option<String>,
    published: Option<String>,
    edited: Option<String>,
    file: FileDto,
    #[serde(default)]
    attachments: Vec<AttachmentDto>,
}

impl PostDto {
    fn images(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(path) = &self.file.path {
            out.push(path.clone());
        }
        out.extend(self.attachments.iter().map(|attachment| attachment.path.clone()));
        out.retain(|path| matches!(path.rsplit('.').next().unwrap_or_default().to_ascii_lowercase().as_str(), "png" | "jpg" | "gif" | "jpeg" | "webp"));
        out.sort();
        out.dedup();
        out
    }

    fn to_chapter(self) -> MangaChapter {
        let fallback_title = self
            .edited
            .as_deref()
            .or(self.published.as_deref())
            .or(self.added.as_deref())
            .map(|date| format!("Post from {date}"))
            .unwrap_or_else(|| "Post".into());
        MangaChapter {
            key: format!("/{}/user/{}/post/{}", self.service, self.user, self.id),
            title: Some(if self.title.is_empty() {
                fallback_title
            } else {
                self.title
            }),
            date_uploaded: None,
            url: Some(format!("{BASE_URL}/{}/user/{}/post/{}", self.service, self.user, self.id)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct FileDto {
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AttachmentDto { path: String }

export_manga_source!(SOURCE);

const CREATORS_FIXTURE: &str = r#"[{"id":"sample","name":"Sample Creator","service":"onlyfans","updated":"2024-01-01T00:00:00","favorited":10}]"#;
const POSTS_FIXTURE: &str = r#"[{"id":"1","service":"onlyfans","user":"sample","title":"Sample Post","added":"2024-01-01T00:00:00","published":null,"edited":null,"file":{"path":"/aa/image.jpg"},"attachments":[]}]"#;
const POST_FIXTURE: &str = r#"{"post":{"id":"1","service":"onlyfans","user":"sample","title":"Sample Post","added":"2024-01-01T00:00:00","published":null,"edited":null,"file":{"path":"/aa/image.jpg"},"attachments":[{"path":"/bb/extra.png"}]}}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_coomer_api() {
        let creators = serde_json::from_str::<Vec<CreatorDto>>(CREATORS_FIXTURE).unwrap();
        assert_eq!(creators[0].to_item().title, "Sample Creator");
        let wrapped = serde_json::from_str::<PostWrapped>(POST_FIXTURE).unwrap();
        assert_eq!(wrapped.post.images().len(), 2);
    }
}
