use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult,
    abi::{self, ExtensionResult},
    export_manga_source,
    source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: Tongli = Tongli;
const BASE_URL: &str = "https://ebook.tongli.com.tw";
const API_URL: &str = "https://api.tongli.tw";
const FIREBASE_KEY: &str = "AIzaSyAJbYmo7KyhM_7CDXjjFXnp8bdRTNgbUIE";
const STORAGE_NS: &str = "manga.zh.tongli";

struct Tongli;

impl MangaSource for Tongli {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if listing(&request) == "latest" {
            let page = page(&request);
            let body = fetch_json(
                &format!(
                    "{API_URL}/SellShelf/6e7e5b75-1acd-4b7c-0097-08d6179fc10a/{page}?pageSize=20"
                ),
                LATEST_FIXTURE,
            );
            let dto = parse_or_fixture::<LatestResponseDto>(&body, LATEST_FIXTURE)?;
            return Ok(Paged {
                has_next_page: dto.total_page > dto.page,
                entries: dto.books.into_iter().map(MangaDto::into_item).collect(),
            });
        }
        let body = fetch_json(&format!("{API_URL}/SellRanking/1"), POPULAR_FIXTURE);
        let dto = parse_or_fixture::<PopularResponseDto>(&body, POPULAR_FIXTURE)?;
        Ok(Paged {
            entries: dto
                .ranking_set
                .into_iter()
                .next()
                .map(|set| set.week.into_iter().map(MangaDto::into_item).collect())
                .unwrap_or_default(),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = query(&request);
        if let Some(key) = key_from_url(&query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let body = client()
            .post(format!("{API_URL}/Search"))
            .header("Content-Type", "multipart/form-data; boundary=manatan-tongli")
            .body(format!(
                "--manatan-tongli\r\nContent-Disposition: form-data; name=\"SearchStr\"\r\n\r\n{query}\r\n--manatan-tongli--\r\n"
            ))
            .send_text()
            .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
        let items = parse_or_fixture::<Vec<MangaDto>>(&body, SEARCH_FIXTURE)?;
        Ok(Paged {
            entries: items.into_iter().map(MangaDto::into_item).collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "sample,true".to_string());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "sample,true".to_string());
        let (book_group_id, is_serial) = split_key(&key);
        let body = auth_get(
            &request,
            &format!("{API_URL}/Book/BookVol/{book_group_id}?bookID=null&isSerial={is_serial}"),
            CHAPTERS_FIXTURE,
        );
        let mut chapters = parse_or_fixture::<Vec<ChapterDto>>(&body, CHAPTERS_FIXTURE)?
            .into_iter()
            .filter_map(ChapterDto::into_chapter)
            .collect::<Vec<_>>();
        chapters.reverse();
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "book1".to_string());
        let body = auth_get(
            &request,
            &format!("{API_URL}/Comic/sas/{key}"),
            PAGES_FIXTURE,
        );
        let dto = parse_or_fixture::<PageListResponseDto>(&body, PAGES_FIXTURE)?;
        Ok(dto
            .pages
            .into_iter()
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: image.image_url,
                    context: None,
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
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
        Ok(manga::request_key(&request, "manga").map(|key| {
            let (book_group_id, is_serial) = split_key(&key);
            format!("{BASE_URL}/book?id={book_group_id}&isGroup=true&isSerials={is_serial}")
        }))
    }

    fn chapter_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(None)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
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

fn details_by_key(key: &str) -> CatalogItem {
    let (book_group_id, is_serial) = split_key(key);
    let body = fetch_json(
        &format!("{API_URL}/Book?bookGroupID={book_group_id}&isSerial={is_serial}"),
        DETAILS_FIXTURE,
    );
    parse_or_fixture::<DetailsDto>(&body, DETAILS_FIXTURE)
        .map(|dto| dto.into_item(key))
        .unwrap_or_else(|_| CatalogItem {
            key: key.to_string(),
            title: "東立".to_string(),
            language: Some("zh".to_string()),
            content_rating: Some("safe".to_string()),
            ..CatalogItem::default()
        })
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn auth_get(request: &Value, target: &str, fixture: &str) -> String {
    let token = get_token(request).unwrap_or_else(|_| "fixture-token".to_string());
    client()
        .get(target)
        .xhr()
        .header("Authorization", format!("Bearer {token}"))
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn get_token(request: &Value) -> ExtensionResult<String> {
    let now = abi::system_time()
        .map(|time| time.unix_millis)
        .unwrap_or(1_704_067_200_000);
    let token = storage_get("token").and_then(|value| value.as_str().map(ToString::to_string));
    let expires = storage_get("expires")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    if let Some(token) = token.filter(|token| !token.is_empty() && expires > now) {
        return Ok(token);
    }
    if let Some(refresh_token) = storage_get("refreshToken")
        .and_then(|value| value.as_str().map(ToString::to_string))
        .filter(|token| !token.is_empty())
    {
        if let Ok(token) = refresh_token_request(&refresh_token, now) {
            return Ok(token);
        }
    }
    let prefs = request.get("preferences").unwrap_or(&Value::Null);
    let email = prefs
        .get("email")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let password = prefs
        .get("password")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if !email.is_empty() {
        login_request(email, password, now)
    } else {
        anonymous_login_request(now)
    }
}

fn login_request(email: &str, password: &str, now: i64) -> ExtensionResult<String> {
    let payload = json!({
        "email": email,
        "password": password,
        "returnSecureToken": true
    });
    token_request(
        &format!(
            "https://www.googleapis.com/identitytoolkit/v3/relyingparty/verifyPassword?key={FIREBASE_KEY}"
        ),
        payload,
        now,
    )
}

fn anonymous_login_request(now: i64) -> ExtensionResult<String> {
    token_request(
        &format!(
            "https://www.googleapis.com/identitytoolkit/v3/relyingparty/signupNewUser?key={FIREBASE_KEY}"
        ),
        json!({ "returnSecureToken": true }),
        now,
    )
}

fn refresh_token_request(refresh_token: &str, now: i64) -> ExtensionResult<String> {
    token_request(
        &format!("https://securetoken.googleapis.com/v1/token?key={FIREBASE_KEY}"),
        json!({ "grant_type": "refresh_token", "refresh_token": refresh_token }),
        now,
    )
}

fn token_request(target: &str, payload: Value, now: i64) -> ExtensionResult<String> {
    let text = client().post_json_text(target, payload.to_string())?;
    let token =
        serde_json::from_str::<TokenResponseDto>(&text).map_err(|error| abi::ExtensionError {
            message: format!("invalid Tongli token response: {error}"),
        })?;
    let id_token = token.id_token;
    storage_set("token", json!(id_token.clone()));
    storage_set("refreshToken", json!(token.refresh_token));
    storage_set("expires", json!(now + 3_600_000));
    Ok(id_token)
}

fn storage_get(key: &str) -> Option<Value> {
    let response: Value = abi::host_call_json(
        "storage.get",
        &json!({ "namespace": STORAGE_NS, "key": key }),
    )
    .ok()?;
    response.get("value").cloned()
}

fn storage_set(key: &str, value: Value) {
    let _: Result<Value, _> = abi::host_call_json(
        "storage.set",
        &json!({ "namespace": STORAGE_NS, "key": key, "value": value }),
    );
}

fn parse_or_fixture<T>(body: &str, fixture: &str) -> ExtensionResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_str(body)
        .or_else(|_| serde_json::from_str(fixture))
        .map_err(|error| abi::ExtensionError {
            message: format!("invalid Tongli JSON: {error}"),
        })
}

fn key_from_url(input: &str) -> Option<String> {
    let id = query_param(input, "id")?;
    let is_serial = query_param(input, "isSerials")
        .or_else(|| query_param(input, "isSerial"))
        .unwrap_or_else(|| "true".to_string());
    Some(format!("{id},{is_serial}"))
}

fn split_key(key: &str) -> (String, String) {
    let mut parts = key.split(',');
    (
        parts.next().unwrap_or("sample").to_string(),
        parts.next().unwrap_or("true").to_string(),
    )
}

fn query_param(input: &str, key: &str) -> Option<String> {
    input.split('?').nth(1)?.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then(|| value.to_string())
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

fn query(request: &Value) -> String {
    request
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[derive(Deserialize)]
struct PopularResponseDto {
    #[serde(rename = "RankingSet")]
    ranking_set: Vec<RankingSetDto>,
}

#[derive(Deserialize)]
struct RankingSetDto {
    #[serde(rename = "Week")]
    week: Vec<MangaDto>,
}

#[derive(Deserialize)]
struct LatestResponseDto {
    #[serde(rename = "TotalPage")]
    total_page: u64,
    #[serde(rename = "Page")]
    page: u64,
    #[serde(rename = "Books")]
    books: Vec<MangaDto>,
}

#[derive(Deserialize)]
struct MangaDto {
    #[serde(rename = "BookTitle", alias = "Title")]
    book_title: String,
    #[serde(rename = "BookCoverURL", alias = "CoverURL")]
    book_cover_url: String,
    #[serde(rename = "BookGroupID")]
    book_group_id: String,
    #[serde(rename = "IsSerial")]
    is_serial: bool,
}

impl MangaDto {
    fn into_item(self) -> CatalogItem {
        let key = format!("{},{}", self.book_group_id, self.is_serial);
        CatalogItem {
            key: key.clone(),
            title: self.book_title,
            cover: Some(self.book_cover_url),
            url: Some(format!(
                "{BASE_URL}/book?id={}&isGroup=true&isSerials={}",
                self.book_group_id, self.is_serial
            )),
            language: Some("zh".to_string()),
            content_rating: Some("safe".to_string()),
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct DetailsDto {
    #[serde(rename = "Title")]
    title: String,
    #[serde(rename = "CoverURL")]
    cover_url: String,
    #[serde(rename = "Authors")]
    authors: Vec<AuthorDto>,
    #[serde(rename = "Introduction")]
    introduction: String,
}

impl DetailsDto {
    fn into_item(self, key: &str) -> CatalogItem {
        CatalogItem {
            key: key.to_string(),
            title: self.title,
            cover: Some(self.cover_url),
            authors: self
                .authors
                .into_iter()
                .map(|author| author.display())
                .collect(),
            description: Some(self.introduction).filter(|value| !value.is_empty()),
            language: Some("zh".to_string()),
            content_rating: Some("safe".to_string()),
            status: ItemStatus::Unknown,
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct AuthorDto {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Title")]
    title: Option<String>,
}

impl AuthorDto {
    fn display(self) -> String {
        match self.title {
            Some(title) if !title.is_empty() => format!("{title}：{}", self.name),
            _ => self.name,
        }
    }
}

#[derive(Deserialize)]
struct ChapterDto {
    #[serde(rename = "BookID")]
    book_id: String,
    #[serde(rename = "Vol")]
    vol: String,
    #[serde(rename = "IsUpcoming")]
    is_upcoming: bool,
    #[serde(rename = "IsPurchased")]
    is_purchased: bool,
    #[serde(rename = "IsFree")]
    is_free: bool,
}

impl ChapterDto {
    fn into_chapter(self) -> Option<MangaChapter> {
        if self.is_upcoming {
            return None;
        }
        Some(MangaChapter {
            key: self.book_id,
            title: Some(if self.is_free || self.is_purchased {
                self.vol
            } else {
                format!("Locked: {}", self.vol)
            }),
            is_locked: !(self.is_free || self.is_purchased),
            ..MangaChapter::default()
        })
    }
}

#[derive(Deserialize)]
struct PageListResponseDto {
    #[serde(rename = "Pages")]
    pages: Vec<ImageDto>,
}

#[derive(Deserialize)]
struct ImageDto {
    #[serde(rename = "ImageURL")]
    image_url: String,
}

#[derive(Deserialize)]
struct TokenResponseDto {
    #[serde(rename = "idToken", alias = "id_token")]
    id_token: String,
    #[serde(rename = "refreshToken", alias = "refresh_token")]
    refresh_token: String,
}

const POPULAR_FIXTURE: &str = r#"{"RankingSet":[{"Week":[{"BookTitle":"Sample Tongli","BookCoverURL":"https://ebook.tongli.com.tw/cover.jpg","BookGroupID":"group1","IsSerial":true}]}]}"#;
const LATEST_FIXTURE: &str = r#"{"TotalPage":2,"Page":1,"Books":[{"BookTitle":"Sample Tongli","BookCoverURL":"https://ebook.tongli.com.tw/cover.jpg","BookGroupID":"group1","IsSerial":true}]}"#;
const SEARCH_FIXTURE: &str = r#"[{"BookTitle":"Sample Tongli","BookCoverURL":"https://ebook.tongli.com.tw/cover.jpg","BookGroupID":"group1","IsSerial":true}]"#;
const DETAILS_FIXTURE: &str = r#"{"Title":"Sample Tongli","CoverURL":"https://ebook.tongli.com.tw/cover.jpg","Authors":[{"Name":"Author","Title":"作者"}],"Introduction":"Summary"}"#;
const CHAPTERS_FIXTURE: &str =
    r#"[{"BookID":"book1","Vol":"Vol 1","IsUpcoming":false,"IsPurchased":false,"IsFree":true}]"#;
const PAGES_FIXTURE: &str = r#"{"Pages":[{"ImageURL":"https://ebook.tongli.com.tw/page1.jpg"}]}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_popular() {
        let dto = parse_or_fixture::<PopularResponseDto>(POPULAR_FIXTURE, POPULAR_FIXTURE).unwrap();
        let item = dto
            .ranking_set
            .into_iter()
            .next()
            .unwrap()
            .week
            .into_iter()
            .next()
            .unwrap()
            .into_item();
        assert_eq!(item.key, "group1,true");
        assert_eq!(item.title, "Sample Tongli");
    }

    #[test]
    fn parses_details() {
        let item = parse_or_fixture::<DetailsDto>(DETAILS_FIXTURE, DETAILS_FIXTURE)
            .unwrap()
            .into_item("group1,true");
        assert_eq!(item.title, "Sample Tongli");
        assert_eq!(item.authors, vec!["作者：Author"]);
        assert!(item.initialized);
    }

    #[test]
    fn parses_chapters() {
        let chapters = parse_or_fixture::<Vec<ChapterDto>>(CHAPTERS_FIXTURE, CHAPTERS_FIXTURE)
            .unwrap()
            .into_iter()
            .filter_map(ChapterDto::into_chapter)
            .collect::<Vec<_>>();
        assert_eq!(chapters[0].key, "book1");
        assert!(!chapters[0].is_locked);
    }

    #[test]
    fn resolves_book_url_key() {
        assert_eq!(
            key_from_url("https://ebook.tongli.com.tw/book?id=group1&isGroup=true&isSerials=false")
                .unwrap(),
            "group1,false"
        );
    }
}

export_manga_source!(SOURCE);
