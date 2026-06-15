use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult,
    abi::{ExtensionError, ExtensionResult},
    export_manga_source,
    source::MangaSource,
};
use manatan_shared::{dates, manga, sdk::http::HttpClient};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: Komiic = Komiic;
const BASE_URL: &str = "https://komiic.com";
const API_URL: &str = "https://komiic.com/api/query";
const PAGE_SIZE: u64 = 30;

struct Komiic;

impl MangaSource for Komiic {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let order_by = if listing(&request) == "popular" {
            "MONTH_VIEWS"
        } else {
            "DATE_UPDATED"
        };
        listing_page(page(&request), order_by, None, None, None)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = query(&request);
        if let Some(id) = id_from_url(&query) {
            return listing_by_ids(&id);
        }
        if let Some(id) = query.strip_prefix("id:").filter(|value| !value.is_empty()) {
            return listing_by_ids(id);
        }
        if !query.is_empty() {
            return search_page(&query);
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        listing_page(
            page(&request),
            filter(filters, "sort").unwrap_or("DATE_UPDATED"),
            filter(filters, "status"),
            filter(filters, "sexyLevel").and_then(|value| value.parse::<u8>().ok()),
            Some(category_ids(filters)),
        )
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        let data = api(manga_query(id_from_key(&key)))?;
        data.comic_by_id
            .map(|dto| dto.into_item())
            .ok_or_else(|| extension_error("Komiic details response did not include comicById"))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        let filter = request
            .get("preferences")
            .and_then(|prefs| prefs.get("chapterFilter"))
            .and_then(Value::as_str)
            .unwrap_or("all");
        let data = api(manga_query(id_from_key(&key)))?;
        let manga_url = data
            .comic_by_id
            .as_ref()
            .map(|comic| comic.url())
            .unwrap_or_else(|| key.clone());
        let mut chapters = data.chapters_by_comic_id.unwrap_or_default();
        if filter != "all" {
            chapters.retain(|chapter| chapter.kind == filter);
        }
        chapters.sort_by(|a, b| {
            b.kind.cmp(&a.kind).then_with(|| {
                serial_sort_number(&b.serial).total_cmp(&serial_sort_number(&a.serial))
            })
        });
        Ok(chapters
            .into_iter()
            .map(|chapter| chapter.into_chapter(&manga_url))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comic/sample/chapter/1".to_string());
        let data = api(page_list_query(id_from_key(&key)))?;
        Ok(data
            .images_by_chapter_id
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(index, image)| {
                let mut headers = manga::image_headers(BASE_URL);
                headers.insert(
                    "accept".to_string(),
                    "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8".to_string(),
                );
                headers.insert(
                    "referer".to_string(),
                    format!("{BASE_URL}{key}/images/all/page/{}", index + 1),
                );
                MangaPage {
                    content: PageContent::Url {
                        url: format!("{BASE_URL}/api/image/{}", image.kid),
                        context: Some(headers),
                    },
                    ..MangaPage::default()
                }
            })
            .collect())
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
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(
            manga::request_key(&request, "chapter")
                .map(|key| format!("{BASE_URL}{key}/images/all")),
        )
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = id_from_url(input) {
            let item = listing_by_ids(&id)?.entries.into_iter().next();
            return Ok(Some(UrlResolveResult {
                item,
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

fn listing_page(
    page: u64,
    order_by: &str,
    status: Option<&str>,
    sexy_level: Option<u8>,
    categories: Option<Vec<String>>,
) -> ExtensionResult<Paged<CatalogItem>> {
    let category_ids = categories.unwrap_or_default();
    if order_by == "MONTH_VIEWS" && !category_ids.is_empty() {
        return Err(extension_error(
            "Komiic monthly views listing cannot filter by category",
        ));
    }
    let data = if order_by == "MONTH_VIEWS" {
        api(popular_query((page - 1) * PAGE_SIZE))?
    } else {
        api(listing_query(
            (page - 1) * PAGE_SIZE,
            order_by,
            status.unwrap_or_default(),
            sexy_level,
            &category_ids,
        ))?
    };
    Ok(data.into_paged())
}

fn search_page(keyword: &str) -> ExtensionResult<Paged<CatalogItem>> {
    Ok(api(search_query(keyword))?.into_paged())
}

fn listing_by_ids(id: &str) -> ExtensionResult<Paged<CatalogItem>> {
    Ok(api(ids_query(id))?.into_paged())
}

fn api(payload: Value) -> ExtensionResult<DataDto> {
    let body = client().post_json_text(API_URL, payload.to_string())?;
    let response: ResponseDto = serde_json::from_str(&body).map_err(extension_error)?;
    response.into_data()
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
}

fn listing_query(
    offset: u64,
    order_by: &str,
    status: &str,
    sexy_level: Option<u8>,
    category_ids: &[String],
) -> Value {
    json!({
        "query": "query comicByCategories($categoryId: [ID!]!, $pagination: Pagination!) { comics: comicByCategories(categoryId: $categoryId, pagination: $pagination) { id title description status imageUrl authors { id name } categories { id name } } allCategory { id name } }",
        "variables": {
            "categoryId": category_ids,
            "pagination": pagination(offset, order_by, status, sexy_level)
        }
    })
}

fn popular_query(offset: u64) -> Value {
    json!({
        "query": "query hotComics($pagination: Pagination!) { comics: hotComics(pagination: $pagination) { id title description status imageUrl authors { id name } categories { id name } } allCategory { id name } }",
        "variables": {
            "pagination": pagination(offset, "MONTH_VIEWS", "", None)
        }
    })
}

fn search_query(keyword: &str) -> Value {
    json!({
        "query": "query searchComicAndAuthorQuery($keyword: String!) { searchComicsAndAuthors(keyword: $keyword) { comics { id title description status imageUrl authors { id name } categories { id name } } } allCategory { id name } }",
        "variables": { "keyword": keyword }
    })
}

fn ids_query(id: &str) -> Value {
    json!({
        "query": "query comicByIds($comicIds: [ID]!) { comics: comicByIds(comicIds: $comicIds) { id title description status imageUrl authors { id name } categories { id name } } }",
        "variables": { "comicIds": [id] }
    })
}

fn manga_query(id: &str) -> Value {
    json!({
        "query": "query chapterByComicId($comicId: ID!) { comicById(comicId: $comicId) { id title description status imageUrl authors { id name } categories { id name } } chaptersByComicId(comicId: $comicId) { id serial type size dateCreated } }",
        "variables": { "comicId": id }
    })
}

fn page_list_query(chapter_id: &str) -> Value {
    json!({
        "query": "query imagesByChapterId($chapterId: ID!) { imagesByChapterId(chapterId: $chapterId) { kid } }",
        "variables": { "chapterId": chapter_id }
    })
}

fn pagination(offset: u64, order_by: &str, status: &str, sexy_level: Option<u8>) -> Value {
    let mut pagination = json!({
        "offset": offset,
        "orderBy": order_by,
        "status": status,
        "asc": false,
        "limit": PAGE_SIZE
    });
    if let Some(level) = sexy_level {
        pagination["sexyLevel"] = json!(level);
    }
    pagination
}

#[derive(Deserialize)]
struct ResponseDto {
    data: Option<DataDto>,
    errors: Option<Vec<ErrorDto>>,
}

impl ResponseDto {
    fn into_data(self) -> ExtensionResult<DataDto> {
        self.data.ok_or_else(|| {
            extension_error(
                self.errors
                    .unwrap_or_default()
                    .into_iter()
                    .map(|error| error.message)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        })
    }
}

#[derive(Deserialize)]
struct ErrorDto {
    message: String,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DataDto {
    comics: Option<Vec<MangaDto>>,
    search_comics_and_authors: Option<SearchDto>,
    comic_by_id: Option<MangaDto>,
    chapters_by_comic_id: Option<Vec<ChapterDto>>,
    images_by_chapter_id: Option<Vec<PageDto>>,
}

impl DataDto {
    fn listing(self) -> Vec<MangaDto> {
        self.comics
            .or_else(|| self.search_comics_and_authors.map(|search| search.comics))
            .unwrap_or_default()
    }

    fn into_paged(self) -> Paged<CatalogItem> {
        let listing = self.listing();
        let has_next_page = listing.len() as u64 == PAGE_SIZE;
        Paged {
            entries: listing.into_iter().map(MangaDto::into_item).collect(),
            has_next_page,
        }
    }
}

#[derive(Deserialize)]
struct SearchDto {
    comics: Vec<MangaDto>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaDto {
    id: String,
    title: String,
    description: String,
    status: String,
    image_url: String,
    authors: Vec<ItemDto>,
    categories: Vec<ItemDto>,
}

impl MangaDto {
    fn url(&self) -> String {
        format!("/comic/{}", self.id)
    }

    fn into_item(self) -> CatalogItem {
        CatalogItem {
            key: self.url(),
            title: self.title,
            cover: Some(self.image_url),
            authors: self.authors.into_iter().map(|item| item.name).collect(),
            tags: self.categories.into_iter().map(|item| item.name).collect(),
            description: (!self.description.is_empty()).then_some(self.description),
            status: match self.status.as_str() {
                "ONGOING" => ItemStatus::Ongoing,
                "END" => ItemStatus::Completed,
                _ => ItemStatus::Unknown,
            },
            url: Some(format!("{BASE_URL}/comic/{}", self.id)),
            language: Some("zh".to_string()),
            content_rating: Some("suggestive".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Clone, Deserialize)]
struct ItemDto {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterDto {
    id: String,
    serial: String,
    #[serde(rename = "type")]
    kind: String,
    size: u64,
    date_created: String,
}

impl ChapterDto {
    fn into_chapter(self, manga_url: &str) -> MangaChapter {
        let (suffix, scanlator) = match self.kind.as_str() {
            "book" => ("卷", "單行本"),
            _ => ("話", "連載"),
        };
        MangaChapter {
            key: format!("{manga_url}/chapter/{}", self.id),
            title: Some(format!("{}{suffix}（{}P）", self.serial, self.size)),
            scanlators: vec![scanlator.to_string()],
            chapter_number: serial_number(&self.serial),
            date_uploaded: self
                .date_created
                .split('T')
                .next()
                .and_then(dates::parse_ymd),
            ..MangaChapter::default()
        }
    }
}

#[derive(Deserialize)]
struct PageDto {
    kid: String,
}

fn serial_number(value: &str) -> Option<f32> {
    value.parse::<f32>().ok()
}

fn serial_sort_number(value: &str) -> f32 {
    serial_number(value).unwrap_or(-1.0)
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

fn query(request: &Value) -> String {
    request
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn filter<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn category_ids(filters: &Value) -> Vec<String> {
    filter(filters, "categoryIds")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn id_from_key(key: &str) -> &str {
    key.trim_end_matches('/').rsplit('/').next().unwrap_or(key)
}

fn id_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(&format!("{BASE_URL}/comic/"))
        .and_then(|rest| rest.split('/').next())
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
}

fn extension_error(error: impl std::fmt::Display) -> ExtensionError {
    ExtensionError {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST_FIXTURE: &str = r#"{"data":{"comics":[{"id":"abc","title":"Sample","description":"Summary","status":"ONGOING","imageUrl":"https://img.example/cover.jpg","authors":[{"id":"a","name":"Author"}],"categories":[{"id":"c","name":"Action"}]}]}}"#;
    const DETAILS_FIXTURE: &str = r#"{"data":{"comicById":{"id":"abc","title":"Sample","description":"Summary","status":"END","imageUrl":"https://img.example/cover.jpg","authors":[{"id":"a","name":"Author"}],"categories":[{"id":"c","name":"Action"}]},"chaptersByComicId":[{"id":"ch1","serial":"1","type":"chapter","size":12,"dateCreated":"2024-01-01T00:00:00Z"},{"id":"b1","serial":"1","type":"book","size":120,"dateCreated":"2024-02-01T00:00:00Z"}]}}"#;
    const PAGES_FIXTURE: &str =
        r#"{"data":{"imagesByChapterId":[{"kid":"page-a"},{"kid":"page-b"}]}}"#;

    #[test]
    fn parses_listing() {
        let data = serde_json::from_str::<ResponseDto>(LIST_FIXTURE)
            .unwrap()
            .into_data()
            .unwrap();
        let page = data.into_paged();
        assert_eq!(page.entries[0].key, "/comic/abc");
        assert_eq!(page.entries[0].authors, vec!["Author"]);
    }

    #[test]
    fn parses_details_and_chapters() {
        let data = serde_json::from_str::<ResponseDto>(DETAILS_FIXTURE)
            .unwrap()
            .into_data()
            .unwrap();
        let item = data.comic_by_id.as_ref().unwrap().clone().into_item();
        assert_eq!(item.status, ItemStatus::Completed);
        let mut chapters = data.chapters_by_comic_id.unwrap();
        chapters.sort_by(|a, b| b.kind.cmp(&a.kind));
        let chapter = chapters.remove(0).into_chapter("/comic/abc");
        assert_eq!(chapter.key, "/comic/abc/chapter/ch1");
        assert_eq!(chapter.date_uploaded, Some(dates::unix_utc_2024_01_01()));
    }

    #[test]
    fn parses_pages() {
        let data = serde_json::from_str::<ResponseDto>(PAGES_FIXTURE)
            .unwrap()
            .into_data()
            .unwrap();
        assert_eq!(data.images_by_chapter_id.unwrap()[0].kid, "page-a");
    }
}

export_manga_source!(SOURCE);
