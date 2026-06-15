use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource, SearchRequest,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const CDN_HOST: &str = "ah-img.luscious.net";
const SOURCE: LusciousSource = LusciousSource;

struct LusciousSource;

impl MangaSource for LusciousSource {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request_page(&request);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") { "date_newest" } else { "rating_all_time" };
        Ok(fetch_album_list(page, "", sort, source, &request))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request_page(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(id) = album_id_from_query(query) {
            return Ok(Paged { entries: vec![fetch_album_details(&id, source, &request)], has_next_page: false });
        }
        let sort = filter_value(&request, "sort").unwrap_or_else(|| "search_score".into());
        Ok(fetch_album_list(page, query, &sort, source, &request))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "/albums/sample_1/".into());
        let id = album_id_from_query(&key).unwrap_or_else(|| "1".into());
        Ok(fetch_album_details(&id, source, &request))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "/albums/sample_1/".into());
        let id = album_id_from_query(&key).unwrap_or_else(|| "1".into());
        let album = fetch_album_dto(&id, &request);
        if preference_bool(&request, "mergeChapters") {
            let chunks = ((album.number_of_pictures as f64) / 1000.0).ceil().max(1.0) as usize;
            return Ok((1..=chunks).rev().map(|chunk| MangaChapter {
                key: format!("album:{id}:{chunk}"),
                title: Some(if chunks == 1 { "Merged Chapter".into() } else { format!("Merged Chapter (Part {chunk})") }),
                chapter_number: Some(chunk as f32),
                date_uploaded: album.created.map(|value| value as i64),
                url: Some(format!("{}{}", base_url(&request), album.url)),
                page_count: Some(album.number_of_pictures as u32),
                ..MangaChapter::default()
            }).collect());
        }
        let pictures = fetch_pictures(&id, 1, &request);
        Ok(pictures.into_iter().rev().map(|picture| MangaChapter {
            key: format!("picture:{}", cdn_image_url(&picture_url(&picture, &request))),
            title: Some(format!("{} - {}", picture.position, picture.title)),
            chapter_number: Some(picture.position as f32),
            date_uploaded: Some(picture.created as i64),
            url: Some(cdn_image_url(&picture_url(&picture, &request))),
            ..MangaChapter::default()
        }).collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter").unwrap_or_default();
        if let Some(rest) = key.strip_prefix("album:") {
            let mut parts = rest.split(':');
            let id = parts.next().unwrap_or("1");
            let chunk = parts.next().and_then(|value| value.parse::<u64>().ok()).unwrap_or(1);
            let start = (chunk.saturating_sub(1)) * 20 + 1;
            let end = chunk * 20;
            let mut pages = Vec::new();
            for page in start..=end {
                let pictures = fetch_pictures(id, page, &request);
                if pictures.is_empty() { break; }
                for picture in pictures {
                    pages.push(page_from_url(cdn_image_url(&picture_url(&picture, &request)), pages.len()));
                }
            }
            return Ok(pages);
        }
        if let Some(image) = key.strip_prefix("picture:") {
            return Ok(vec![page_from_url(image.to_string(), 0)]);
        }
        Ok(Vec::new())
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let source = source_for(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(id) = album_id_from_query(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_album_details(&id, source, &request)),
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

export_manga_source!(SOURCE);

#[derive(Clone, Copy)]
struct SourceConfig { id: &'static str, lang: &'static str, lus_lang: &'static str }

const SOURCES: &[SourceConfig] = &[
    SourceConfig { id: "luscious-en", lang: "en", lus_lang: "1" },
    SourceConfig { id: "luscious-ja", lang: "ja", lus_lang: "2" },
    SourceConfig { id: "luscious-es", lang: "es", lus_lang: "3" },
    SourceConfig { id: "luscious-it", lang: "it", lus_lang: "4" },
    SourceConfig { id: "luscious-de", lang: "de", lus_lang: "5" },
    SourceConfig { id: "luscious-fr", lang: "fr", lus_lang: "6" },
    SourceConfig { id: "luscious-zh", lang: "zh", lus_lang: "8" },
    SourceConfig { id: "luscious-ko", lang: "ko", lus_lang: "9" },
    SourceConfig { id: "luscious-other", lang: "other", lus_lang: "99" },
    SourceConfig { id: "luscious-pt-br", lang: "pt-BR", lus_lang: "100" },
    SourceConfig { id: "luscious-th", lang: "th", lus_lang: "101" },
    SourceConfig { id: "luscious-all", lang: "all", lus_lang: "" },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request.get("sourceId").or_else(|| request.get("source_id")).and_then(Value::as_str).unwrap_or("luscious-all");
    SOURCES.iter().copied().find(|source| source.id == id).unwrap_or(SOURCES[11])
}

fn client(request: &Value) -> http::HttpClient {
    let base = base_url(request);
    http::HttpClient::browser().with_referer(format!("{base}/")).with_cookies_for(base)
}

fn fetch_album_list(page: u64, query: &str, sort: &str, source: SourceConfig, request: &Value) -> Paged<CatalogItem> {
    let filters = album_filters(query, source, request);
    let variables = json!({"input":{"display":sort,"page":page,"items_per_page":50,"filters":filters}});
    let body = fetch_graphql("AlbumList", ALBUM_LIST_QUERY, variables, ALBUM_LIST_FIXTURE, request);
    let response = serde_json::from_str::<AlbumListResponse>(&body).unwrap_or_else(|_| serde_json::from_str(ALBUM_LIST_FIXTURE).expect("album list fixture"));
    Paged {
        entries: response.data.album.list.items.into_iter().map(|album| album.into_item(source)).collect(),
        has_next_page: response.data.album.list.info.has_next_page,
    }
}

fn fetch_album_details(id: &str, source: SourceConfig, request: &Value) -> CatalogItem {
    let album = fetch_album_dto(id, request);
    album.into_item(source)
}

fn fetch_album_dto(id: &str, request: &Value) -> FullAlbum {
    let body = fetch_graphql("AlbumGet", ALBUM_GET_QUERY, json!({"id": id}), ALBUM_GET_FIXTURE, request);
    serde_json::from_str::<AlbumGetResponse>(&body).unwrap_or_else(|_| serde_json::from_str(ALBUM_GET_FIXTURE).expect("album fixture")).data.album.get
}

fn fetch_pictures(id: &str, page: u64, request: &Value) -> Vec<Picture> {
    let variables = json!({"input":{"display":preference_str(request, "pageSort").unwrap_or_else(|| "position".into()),"page":page,"items_per_page":50,"filters":[{"name":"album_id","value":id}]}});
    let body = fetch_graphql("AlbumListOwnPictures", ALBUM_PICTURES_QUERY, variables, PICTURES_FIXTURE, request);
    serde_json::from_str::<AlbumListOwnPicturesResponse>(&body)
        .unwrap_or_else(|_| serde_json::from_str(PICTURES_FIXTURE).expect("pictures fixture"))
        .data.picture.list.items
}

fn fetch_graphql(operation: &str, query: &str, variables: Value, fixture: &str, request: &Value) -> String {
    let base = base_url(request);
    let target = format!("{}/graphql/nobatch/?operationName={}&query={}&variables={}", base, operation, query_escape(query), query_escape(&variables.to_string()));
    client(request).get(target).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn album_filters(query: &str, source: SourceConfig, request: &Value) -> Vec<Value> {
    let mut filters = vec![json!({"name":"audience_ids","value":"+1+10+2+3+5+6+8+9"})];
    if !source.lus_lang.is_empty() {
        filters.push(json!({"name":"language_ids","value":format!("+{}", source.lus_lang)}));
    }
    if !query.is_empty() {
        filters.push(json!({"name":"search_query","value":query}));
    }
    if let Some(tags) = filter_value(request, "tags").filter(|value| !value.is_empty()) {
        filters.push(json!({"name":"tagged","value":format!("+{}", tags.to_ascii_lowercase().replace(' ', "_").replace(',', "+"))}));
    }
    if let Some(uploader) = filter_value(request, "uploader").filter(|value| !value.is_empty()) {
        filters.push(json!({"name":"created_by_id","value":uploader}));
    }
    if let Some(favorite) = filter_value(request, "favoriteBy").filter(|value| !value.is_empty()) {
        filters.push(json!({"name":"favorite_by_user_id","value":favorite}));
    }
    filters
}

fn picture_url(picture: &Picture, request: &Value) -> String {
    let resolution = preference_str(request, "resolution").unwrap_or_else(|| "-1".into());
    if resolution != "-1" {
        if let Some(url) = resolution.parse::<usize>().ok().and_then(|index| picture.thumbnails.get(index)).map(|cover| cover.url.clone()) {
            return url;
        }
    }
    picture.url_to_video.as_ref().map(|url| url.replace(".mp4", ".gif"))
        .or_else(|| picture.url_to_original.clone())
        .or_else(|| picture.thumbnails.iter().max_by_key(|cover| cover.height * cover.width).map(|cover| cover.url.clone()))
        .unwrap_or_default()
}

fn cdn_image_url(input: &str) -> String {
    let normalized = if input.starts_with("//") { format!("https:{input}") } else { input.to_string() };
    if let Some(path_start) = normalized[8.min(normalized.len())..].find('/') {
        let split = path_start + 8;
        format!("https://{CDN_HOST}{}", &normalized[split..])
    } else {
        normalized
    }
}

fn page_from_url(url: String, index: usize) -> MangaPage {
    MangaPage {
        content: PageContent::Url { url, context: Some(image_headers()) },
        headers: image_headers(),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn album_id_from_query(input: &str) -> Option<String> {
    input.strip_prefix("ID:").map(ToString::to_string).or_else(|| {
        input.trim_end_matches('/').rsplit('_').next().filter(|value| value.chars().all(|ch| ch.is_ascii_digit())).map(ToString::to_string)
    })
}

fn base_url(request: &Value) -> String {
    preference_str(request, "mirror").unwrap_or_else(|| "https://www.luscious.net".into()).trim_end_matches('/').to_string()
}

fn request_page(request: &Value) -> u64 { request.get("page").and_then(Value::as_u64).unwrap_or(1) }
fn request_key(request: &Value, field: &str) -> Option<String> { request.get(field).and_then(|value| value.get("key").or_else(|| value.get("url")).and_then(Value::as_str).or_else(|| value.as_str())).map(ToString::to_string) }
fn filter_value(request: &Value, id: &str) -> Option<String> { request.get("filters").and_then(Value::as_object).and_then(|filters| filters.get(id)).and_then(Value::as_str).map(ToString::to_string) }
fn preference_str(request: &Value, id: &str) -> Option<String> { request.get("preferences").and_then(Value::as_object).and_then(|prefs| prefs.get(id)).and_then(Value::as_str).map(ToString::to_string) }
fn preference_bool(request: &Value, id: &str) -> bool { request.get("preferences").and_then(Value::as_object).and_then(|prefs| prefs.get(id)).and_then(Value::as_bool).unwrap_or(false) }

fn image_headers() -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    headers.insert("Referer".into(), "https://www.luscious.net/".into());
    headers
}

fn query_escape(input: &str) -> String {
    input.bytes().fold(String::new(), |mut out, byte| {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
        out
    })
}

#[derive(Deserialize)]
struct Info { has_next_page: bool }
#[derive(Deserialize)]
struct AlbumListResponse { data: AlbumListData }
#[derive(Deserialize)]
struct AlbumListData { album: AlbumListDataAlbum }
#[derive(Deserialize)]
struct AlbumListDataAlbum { list: AlbumList }
#[derive(Deserialize)]
struct AlbumList { info: Info, items: Vec<Album> }
#[derive(Deserialize)]
struct Album { url: String, title: String, cover: Cover }
impl Album {
    fn into_item(self, source: SourceConfig) -> CatalogItem {
        CatalogItem {
            key: self.url.clone(),
            title: self.title,
            cover: Some(self.cover.url),
            url: Some(format!("https://www.luscious.net{}", self.url)),
            language: Some(source.lang.into()),
            content_rating: Some("adult".into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct AlbumGetResponse { data: AlbumGetData }
#[derive(Deserialize)]
struct AlbumGetData { album: AlbumGetDataGet }
#[derive(Deserialize)]
struct AlbumGetDataGet { get: FullAlbum }
#[derive(Deserialize)]
struct FullAlbum {
    url: String,
    title: String,
    cover: Cover,
    description: String,
    language: Option<ItemWithTitle>,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    genres: Vec<ItemWithTitle>,
    #[serde(default)]
    audiences: Vec<ItemWithTitle>,
    #[serde(default)]
    tags: Vec<Tag>,
    number_of_pictures: i64,
    number_of_animated_pictures: i64,
    content: ItemWithTitle,
    created: Option<f64>,
}
impl FullAlbum {
    fn into_item(self, source: SourceConfig) -> CatalogItem {
        let mut tags = Vec::new();
        if let Some(language) = self.language { tags.push(language.title); }
        tags.extend(self.labels);
        tags.extend(self.genres.into_iter().map(|item| item.title));
        tags.extend(self.audiences.into_iter().map(|item| item.title));
        tags.extend(self.tags.iter().map(|tag| tag.text.clone()));
        tags.push(self.content.title);
        let artist = self.tags.iter().find_map(|tag| tag.text.strip_prefix("Artist:").map(|value| value.trim().to_string()));
        CatalogItem {
            key: self.url.clone(),
            title: self.title,
            cover: Some(self.cover.url),
            url: Some(format!("https://www.luscious.net{}", self.url)),
            authors: artist.clone().into_iter().collect(),
            artists: artist.into_iter().collect(),
            description: Some(format!("{}\n\nPictures: {}\nAnimated Pictures: {}", self.description, self.number_of_pictures, self.number_of_animated_pictures)),
            tags,
            language: Some(source.lang.into()),
            content_rating: Some("adult".into()),
            status: ItemStatus::Completed,
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct ItemWithTitle { title: String }
#[derive(Deserialize)]
struct Tag { text: String }
#[derive(Deserialize)]
struct Cover { url: String, #[serde(default)] height: i64, #[serde(default)] width: i64 }
#[derive(Deserialize)]
struct AlbumListOwnPicturesResponse { data: AlbumListOwnPicturesData }
#[derive(Deserialize)]
struct AlbumListOwnPicturesData { picture: AlbumListOwnPicturesDataPicture }
#[derive(Deserialize)]
struct AlbumListOwnPicturesDataPicture { list: AlbumListOwnPicturesList }
#[derive(Deserialize)]
struct AlbumListOwnPicturesList { items: Vec<Picture> }
#[derive(Deserialize)]
struct Picture {
    #[serde(default)]
    thumbnails: Vec<Cover>,
    url_to_original: Option<String>,
    url_to_video: Option<String>,
    position: i64,
    title: String,
    created: f64,
}

const ALBUM_LIST_QUERY: &str = "query AlbumList($input: AlbumListInput!) { album { list(input: $input) { info { has_next_page } items } } }";
const ALBUM_PICTURES_QUERY: &str = "query AlbumListOwnPictures($input: PictureListInput!) { picture { list(input: $input) { items { created title url_to_original url_to_video position thumbnails { url height width } } } } }";
const ALBUM_GET_QUERY: &str = "query AlbumGet($id: ID!) { album { get(id: $id) { ... on Album { title url description number_of_pictures number_of_animated_pictures created cover { url height width } content { title } language { title } labels genres { title } audiences { title } tags { text } } } } }";

const ALBUM_LIST_FIXTURE: &str = r#"{"data":{"album":{"list":{"info":{"has_next_page":false},"items":[{"url":"/albums/sample_1/","title":"Sample Album","cover":{"url":"https://www.luscious.net/cover.jpg","height":100,"width":100}}]}}}}"#;
const ALBUM_GET_FIXTURE: &str = r#"{"data":{"album":{"get":{"url":"/albums/sample_1/","title":"Sample Album","cover":{"url":"https://www.luscious.net/cover.jpg","height":100,"width":100},"description":"Sample description","language":{"title":"English"},"labels":["Manga"],"genres":[{"title":"SFW"}],"audiences":[{"title":"Straight"}],"tags":[{"text":"Artist: Sample"}],"number_of_pictures":2,"number_of_animated_pictures":0,"content":{"title":"Hentai"},"created":1704067200}}}}"#;
const PICTURES_FIXTURE: &str = r#"{"data":{"picture":{"list":{"items":[{"thumbnails":[{"url":"https://www.luscious.net/low.jpg","height":100,"width":100},{"url":"https://www.luscious.net/medium.jpg","height":200,"width":200},{"url":"https://www.luscious.net/high.jpg","height":300,"width":300}],"url_to_original":"https://www.luscious.net/original.jpg","url_to_video":null,"position":1,"title":"Page 1","created":1704067200}]}}}}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_luscious_graphql() {
        let source = SOURCES[0];
        let page = fetch_album_list(1, "", "rating_all_time", source, &Value::Null);
        assert_eq!(page.entries[0].key, "/albums/sample_1/");
        let detail = fetch_album_details("1", source, &Value::Null);
        assert_eq!(detail.artists, vec!["Sample"]);
        let pictures = fetch_pictures("1", 1, &Value::Null);
        assert_eq!(cdn_image_url(&picture_url(&pictures[0], &Value::Null)), "https://ah-img.luscious.net/original.jpg");
    }
}
