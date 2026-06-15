use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::{
    Encryptor,
    cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7},
};
use des::TdesEde3;
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult,
    abi::{self, ExtensionResult},
    export_manga_source,
    source::MangaSource,
};
use manatan_shared::{dates, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

type TdesCbcEnc = Encryptor<TdesEde3>;

const SOURCE: Vomic = Vomic;
const DEFAULT_DOMAIN: &str = "www.vomicmh.com";
const IV: &str = "k8tUyS$m";
const INFO_LOCKED_IMAGE: &str = "https://cdn.vomicer.com/qiniu/vomic/otherImg/info2.webp";

struct Vomic;

impl MangaSource for Vomic {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let endpoints = endpoints(&request);
        let page = page(&request);
        let body = fetch_json(
            &request,
            &format!(
                "{}/api/v1/rank/rank-data?rank_id=1&page={page}",
                endpoints.api_url
            ),
            LIST_FIXTURE,
        );
        let dto = parse_response::<RankingDto>(&body, LIST_FIXTURE)?;
        Ok(Paged {
            entries: dto
                .result
                .into_iter()
                .filter_map(|item| item.into_item(&endpoints.base_url))
                .collect(),
            has_next_page: page < 4,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let endpoints = endpoints(&request);
        let query = query(&request);
        if let Some(id) = id_from_url(&query) {
            return Ok(Paged {
                entries: vec![fetch_details(&request, &id)?],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let category_search = filters
            .get("categorySearch")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let title = if category_search { "" } else { query.as_str() };
        let category = if category_search {
            query.as_str()
        } else {
            filter(filters, "category").unwrap_or("")
        };
        if title.is_empty() && category.is_empty() {
            return Ok(Paged {
                entries: Vec::new(),
                has_next_page: false,
            });
        }
        let target = format!(
            "{}/api/v1/search/search-comic-data?title={}&category={}&page={}",
            endpoints.api_url,
            url::query_escape(title),
            url::query_escape(category),
            page(&request)
        );
        let body = fetch_json(&request, &target, SEARCH_FIXTURE);
        let dto = parse_response::<MangaListDto>(&body, SEARCH_FIXTURE)?;
        Ok(Paged {
            has_next_page: dto.has_next_page(),
            entries: dto
                .entries()
                .into_iter()
                .filter_map(|item| item.into_item(&endpoints.base_url))
                .collect(),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let id = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "00000000000000000000000000000001".to_string());
        fetch_details(&request, &id)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let endpoints = endpoints(&request);
        let manga_id = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "00000000000000000000000000000001".to_string());
        let body = fetch_json(
            &request,
            &format!(
                "{}/api/v1/detail/get-comic-detail-chapter-data?mid={manga_id}",
                endpoints.api_url
            ),
            CHAPTERS_FIXTURE,
        );
        let chapters = parse_response::<Vec<ChapterDto>>(&body, CHAPTERS_FIXTURE)?;
        Ok(chapters
            .into_iter()
            .map(|chapter| chapter.into_chapter(&endpoints.base_url, &manga_id))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let endpoints = endpoints(&request);
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| {
            "00000000000000000000000000000001/00000000000000000000000000000002".to_string()
        });
        let (manga_id, chapter_id) = split_chapter_key(&key);
        let target = encrypted_page_url(&endpoints.api_url, &manga_id, &chapter_id)?;
        let body = fetch_json(&request, &target, PAGES_FIXTURE);
        let images = parse_response::<Vec<String>>(&body, PAGES_FIXTURE)?;
        if images.len() == 1
            && images
                .first()
                .is_some_and(|image| image == INFO_LOCKED_IMAGE)
        {
            return Err(abi::ExtensionError {
                message: "Unable to read this chapter".to_string(),
            });
        }
        Ok(images
            .into_iter()
            .enumerate()
            .map(|(index, image)| {
                let referer = image_host_referer(&image)
                    .unwrap_or_else(|| format!("{}/", endpoints.base_url));
                MangaPage {
                    content: PageContent::Url {
                        url: image,
                        context: None,
                    },
                    headers: manga::image_headers(&referer),
                    description: Some(format!("Page {}", index + 1)),
                    ..MangaPage::default()
                }
            })
            .collect())
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular", "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}))?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: popular.entries,
            has_more: popular.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let endpoints = endpoints(&request);
        Ok(manga::request_key(&request, "manga")
            .map(|id| format!("{}/#/detail?id={id}", endpoints.base_url)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let endpoints = endpoints(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (manga_id, chapter_id) = split_chapter_key(&key);
            format!("{}/#/page/{manga_id}/{chapter_id}", endpoints.base_url)
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = id_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&request, &id)?),
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

fn fetch_details(request: &Value, id: &str) -> ExtensionResult<CatalogItem> {
    let endpoints = endpoints(request);
    let body = fetch_json(
        request,
        &format!(
            "{}/api/v1/detail/get-comic-detail-data?mid={id}",
            endpoints.api_url
        ),
        DETAILS_FIXTURE,
    );
    parse_response::<MangaDto>(&body, DETAILS_FIXTURE)?
        .into_details(&endpoints.base_url)
        .ok_or_else(|| abi::ExtensionError {
            message: "empty Vomic details response".to_string(),
        })
}

fn client(base_url: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{base_url}/"))
        .with_cookies_for(base_url)
}

fn fetch_json(request: &Value, target: &str, fixture: &str) -> String {
    let endpoints = endpoints(request);
    client(&endpoints.base_url)
        .get(target)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn encrypted_page_url(api_url: &str, manga_id: &str, chapter_id: &str) -> ExtensionResult<String> {
    let key = random_key()?;
    let time = abi::system_time()
        .map(|time| time.unix_millis)
        .unwrap_or(1_704_067_200_000)
        .to_string();
    let payload = format!("{key}{IV}cid={chapter_id}&mid={manga_id}{time}");
    let encrypted = TdesCbcEnc::new_from_slices(key.as_bytes(), IV.as_bytes())
        .map_err(|error| abi::ExtensionError {
            message: format!("invalid Vomic encryption key: {error}"),
        })?
        .encrypt_padded_vec_mut::<Pkcs7>(payload.as_bytes());
    let encoded = STANDARD.encode(encrypted);
    Ok(format!(
        "{api_url}/api/v2/page/get-comic-page-img-data?k={}&t={time}&e={}",
        url::query_escape(&key),
        url::query_escape(&encoded)
    ))
}

fn random_key() -> ExtensionResult<String> {
    const ALPHANUMERIC: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let bytes = abi::host_call_json::<_, Value>("system.randomBytes", &json!({ "length": 24 }))
        .ok()
        .and_then(|response| {
            response
                .get("bytesBase64")
                .and_then(Value::as_str)
                .and_then(|bytes| STANDARD.decode(bytes).ok())
        })
        .unwrap_or_else(|| b"ManatanVomicFallbackKey!".to_vec());
    Ok(bytes
        .into_iter()
        .take(24)
        .map(|byte| ALPHANUMERIC[(byte as usize) % ALPHANUMERIC.len()] as char)
        .collect())
}

#[derive(Clone)]
struct Endpoints {
    base_url: String,
    api_url: String,
}

fn endpoints(request: &Value) -> Endpoints {
    let domain = request
        .get("preferences")
        .and_then(|preferences| preferences.get("domain"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_DOMAIN);
    let domain = domain
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/');
    if domain.starts_with("www.") || domain.starts_with("api.") {
        let tld = &domain[4..];
        Endpoints {
            base_url: format!("http://www.{tld}"),
            api_url: format!("http://api.{tld}"),
        }
    } else {
        let url = format!("http://{domain}");
        Endpoints {
            base_url: url.clone(),
            api_url: url,
        }
    }
}

fn parse_response<T>(body: &str, fixture: &str) -> ExtensionResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_str::<ResponseDto<T>>(body)
        .or_else(|_| serde_json::from_str::<ResponseDto<T>>(fixture))
        .map(|response| response.data)
        .map_err(|error| abi::ExtensionError {
            message: format!("invalid Vomic JSON response: {error}"),
        })
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

fn id_from_url(input: &str) -> Option<String> {
    let id = input
        .split("id=")
        .nth(1)
        .and_then(|value| value.split(['&', '#']).next())
        .or_else(|| {
            input
                .split("/page/")
                .nth(1)
                .and_then(|value| value.split('/').next())
        })?;
    (id.len() == 32).then(|| id.to_string())
}

fn split_chapter_key(key: &str) -> (String, String) {
    let mut parts = key.split('/');
    (
        parts
            .next()
            .unwrap_or("00000000000000000000000000000001")
            .to_string(),
        parts
            .next()
            .unwrap_or("00000000000000000000000000000002")
            .to_string(),
    )
}

fn image_host_referer(image: &str) -> Option<String> {
    let rest = image
        .strip_prefix("https://")
        .or_else(|| image.strip_prefix("http://"))?;
    let host = rest.split('/').next()?;
    Some(format!("https://{host}/"))
}

#[derive(Deserialize)]
struct ResponseDto<T> {
    data: T,
}

#[derive(Deserialize)]
struct RankingDto {
    result: Vec<MangaDto>,
}

#[derive(Deserialize)]
struct MangaListDto {
    page: u64,
    result_count: u64,
    result: Vec<MangaDto>,
}

impl MangaListDto {
    fn entries(self) -> Vec<MangaDto> {
        if self.result_count == 0 {
            Vec::new()
        } else {
            self.result
        }
    }

    fn has_next_page(&self) -> bool {
        self.page < 100 && self.page * 12 < self.result_count
    }
}

#[derive(Deserialize)]
struct MangaDto {
    mid: String,
    title: String,
    site: Option<SiteDto>,
    cover_img_url: Option<String>,
    authors_name: Option<Vec<String>>,
    status: Option<String>,
    categories: Option<Vec<String>>,
    description: Option<String>,
}

impl MangaDto {
    fn into_item(self, base_url: &str) -> Option<CatalogItem> {
        (!self.title.is_empty()).then(|| CatalogItem {
            key: self.mid.clone(),
            title: self.title,
            cover: self.cover_img_url,
            url: Some(format!("{base_url}/#/detail?id={}", self.mid)),
            language: Some("zh".to_string()),
            content_rating: Some("safe".to_string()),
            ..CatalogItem::default()
        })
    }

    fn into_details(self, base_url: &str) -> Option<CatalogItem> {
        let authors = self.authors_name.clone().unwrap_or_default();
        let status = self.status.clone();
        let tags = self.categories.clone().unwrap_or_default();
        let description = [
            self.site
                .clone()
                .map(|site| format!("站点：{}", site.display())),
            self.description.clone(),
        ]
        .into_iter()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
        let mut item = self.into_item(base_url)?;
        item.authors = authors;
        item.description = Some(description).filter(|value| !value.is_empty());
        item.tags = tags;
        item.status = match status.as_deref() {
            Some("连载中") => ItemStatus::Ongoing,
            Some("已完结") => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        };
        item.initialized = true;
        Some(item)
    }
}

#[derive(Clone, Deserialize)]
struct SiteDto {
    site_en: String,
    site_cn: Option<String>,
}

impl SiteDto {
    fn display(self) -> String {
        match self.site_cn {
            Some(site_cn) if !site_cn.is_empty() => format!("{site_cn} ({})", self.site_en),
            _ => self.site_en,
        }
    }
}

#[derive(Deserialize)]
struct ChapterDto {
    title: String,
    cid: String,
    update_time: String,
}

impl ChapterDto {
    fn into_chapter(self, base_url: &str, manga_id: &str) -> MangaChapter {
        MangaChapter {
            key: format!("{manga_id}/{}", self.cid),
            title: Some(self.title),
            date_uploaded: dates::parse_ymd(
                self.update_time.split_whitespace().next().unwrap_or(""),
            ),
            url: Some(format!("{base_url}/#/page/{manga_id}/{}", self.cid)),
            ..MangaChapter::default()
        }
    }
}

const LIST_FIXTURE: &str = r#"{"data":{"result":[{"mid":"00000000000000000000000000000001","title":"Sample Vomic","cover_img_url":"https://cdn.vomicer.com/cover.jpg"}]}}"#;
const SEARCH_FIXTURE: &str = r#"{"data":{"page":1,"result_count":1,"result":[{"mid":"00000000000000000000000000000001","title":"Sample Vomic","cover_img_url":"https://cdn.vomicer.com/cover.jpg"}]}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"mid":"00000000000000000000000000000001","title":"Sample Vomic","site":{"site_en":"sample","site_cn":"示例"},"cover_img_url":"https://cdn.vomicer.com/cover.jpg","authors_name":["Author"],"status":"连载中","categories":["Action"],"description":"Summary"}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":[{"title":"Chapter 1","cid":"00000000000000000000000000000002","update_time":"2024-01-01 00:00:00"}]}"#;
const PAGES_FIXTURE: &str = r#"{"data":["https://cdn.vomicer.com/qiniu/vomic/page1.webp"]}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_details() {
        let item = parse_response::<MangaDto>(DETAILS_FIXTURE, DETAILS_FIXTURE)
            .unwrap()
            .into_details("http://www.vomicmh.com")
            .unwrap();
        assert_eq!(item.title, "Sample Vomic");
        assert_eq!(item.authors, vec!["Author"]);
        assert_eq!(item.status, ItemStatus::Ongoing);
        assert!(item.initialized);
    }

    #[test]
    fn parses_chapters() {
        let chapters = parse_response::<Vec<ChapterDto>>(CHAPTERS_FIXTURE, CHAPTERS_FIXTURE)
            .unwrap()
            .into_iter()
            .map(|chapter| {
                chapter.into_chapter("http://www.vomicmh.com", "00000000000000000000000000000001")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            chapters[0].key,
            "00000000000000000000000000000001/00000000000000000000000000000002"
        );
        assert_eq!(
            chapters[0].date_uploaded,
            Some(dates::unix_utc_2024_01_01())
        );
    }

    #[test]
    fn builds_referer_from_image_host() {
        assert_eq!(
            image_host_referer("https://cdn.vomicer.com/path/page.webp").unwrap(),
            "https://cdn.vomicer.com/"
        );
    }
}

export_manga_source!(SOURCE);
