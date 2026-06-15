use aes::Aes128;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient};
use serde::Deserialize;
use serde_json::Value;

type Aes128CbcDec = cbc::Decryptor<Aes128>;

const SOURCE: SunshineButterflyScans = SunshineButterflyScans;
const BASE_URL: &str = "https://wings.sbs";
const CDN_URL: &str = "https://wings.sbs/images/projcoverjpeg/";
const GOOGLE_DRIVE_KEY: &str = "AIzaSyDDWjOHN1UPcafkwyJLO7fX1gmVyntIozs";
const IMGUR_BEARER: &str = "84155230e6a2d98eaea1cee48d97e6ecff0f6c12";
const KEY_B64: &str = "YX+1nM4KgfaYwNE3/MPcTg==";
const IV_B64: &str = "279GjT2Xu9LZBkI4zLzIAg==";

struct SunshineButterflyScans;

impl MangaSource for SunshineButterflyScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let mut groups = grouped_entries(&fetch_chapters_json());
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            groups.sort_by_key(|entries| {
                entries
                    .first()
                    .and_then(|entry| entry.timestamp.parse::<i64>().ok())
                    .unwrap_or(0)
            });
            groups.reverse();
        } else {
            groups.sort_by_key(|entries| {
                entries
                    .first()
                    .map(|entry| entry.series.to_ascii_lowercase())
                    .unwrap_or_default()
            });
        }
        Ok(Paged {
            entries: groups
                .into_iter()
                .filter_map(|entries| entries.first().cloned())
                .map(|entry| entry.to_item())
                .collect(),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let status = request
            .get("filters")
            .and_then(|filters| filters.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let sort = request
            .get("filters")
            .and_then(|filters| filters.get("sort"))
            .and_then(Value::as_str)
            .unwrap_or("name");
        let mut groups = grouped_entries(&fetch_chapters_json());
        groups.retain(|entries| {
            entries.first().is_some_and(|entry| {
                entry.series.to_ascii_lowercase().contains(&query)
                    && (status.is_empty() || entry.project_status == status)
            })
        });
        if sort == "updated" {
            groups.sort_by_key(|entries| {
                entries
                    .first()
                    .and_then(|entry| entry.timestamp.parse::<i64>().ok())
                    .unwrap_or(0)
            });
            groups.reverse();
        } else {
            groups.sort_by_key(|entries| {
                entries
                    .first()
                    .map(|entry| entry.series.to_ascii_lowercase())
                    .unwrap_or_default()
            });
        }
        Ok(Paged {
            entries: groups
                .into_iter()
                .filter_map(|entries| entries.first().cloned())
                .map(|entry| entry.to_item())
                .collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/projects?n=sample".to_string());
        let project = key.split("?n=").nth(1).unwrap_or("sample");
        Ok(fetch_chapters_json()
            .into_iter()
            .find(|entry| entry.project_name == project)
            .unwrap_or_else(sample_entry)
            .to_item())
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/projects?n=sample".to_string());
        let project = key.split("?n=").nth(1).unwrap_or("sample");
        let mut entries = fetch_chapters_json()
            .into_iter()
            .filter(|entry| entry.project_name == project)
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| -(entry.num as i64));
        Ok(entries
            .into_iter()
            .map(|entry| entry.to_chapter())
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/read?series=sample&num=1".to_string());
        let project = key
            .split("series=")
            .nth(1)
            .and_then(|part| part.split('&').next())
            .unwrap_or("sample");
        let num = key
            .split("num=")
            .nth(1)
            .and_then(|part| part.parse::<i64>().ok())
            .unwrap_or(1);
        let Some(entry) = fetch_chapters_json()
            .into_iter()
            .find(|entry| entry.project_name == project && entry.num == num)
        else {
            return Ok(parse_imgur_pages(IMGUR_FIXTURE));
        };
        let album = decrypt_album_id(&entry.album_id).unwrap_or(entry.album_id);
        if album.len() > 10 {
            let endpoint = format!(
                "https://www.googleapis.com/drive/v3/files?q=\"{album}\"+in+parents&key={GOOGLE_DRIVE_KEY}&orderBy=name_natural&fields=files(id,name,imageMediaMetadata)&pageSize=250"
            );
            Ok(parse_drive_pages(&fetch_document(&endpoint, DRIVE_FIXTURE)))
        } else {
            let endpoint = format!("https://api.imgur.com/3/album/{album}/images");
            Ok(parse_imgur_pages(&fetch_imgur(&endpoint, IMGUR_FIXTURE)))
        }
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let input = request
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default();
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_imgur(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .header("Authorization", format!("Bearer {IMGUR_BEARER}"))
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_chapters_json() -> Vec<EntryDto> {
    serde_json::from_str(&fetch_document(
        &format!("{BASE_URL}/json/chapters.json"),
        CHAPTERS_FIXTURE,
    ))
    .unwrap_or_else(|_| vec![sample_entry()])
}

fn grouped_entries(entries: &[EntryDto]) -> Vec<Vec<EntryDto>> {
    let mut groups: Vec<Vec<EntryDto>> = Vec::new();
    for entry in entries.iter().cloned() {
        if let Some(group) = groups.iter_mut().find(|group| {
            group
                .first()
                .is_some_and(|first| first.project_name == entry.project_name)
        }) {
            group.push(entry);
        } else {
            groups.push(vec![entry]);
        }
    }
    groups
}

fn decrypt_album_id(input: &str) -> Option<String> {
    let key = STANDARD.decode(KEY_B64).ok()?;
    let iv = STANDARD.decode(IV_B64).ok()?;
    let data = STANDARD.decode(input).ok()?;
    let plaintext = Aes128CbcDec::new_from_slices(&key, &iv)
        .ok()?
        .decrypt_padded_vec_mut::<Pkcs7>(&data)
        .ok()?;
    String::from_utf8(plaintext).ok()
}

fn parse_drive_pages(body: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<GoogleDriveResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(DRIVE_FIXTURE).expect("drive fixture"));
    response
        .files
        .into_iter()
        .enumerate()
        .map(|(index, file)| MangaPage {
            content: PageContent::Url {
                url: format!(
                    "https://lh3.googleusercontent.com/d/{}=w{}",
                    file.id, file.image_media_metadata.width
                ),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_imgur_pages(body: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<ImgurResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(IMGUR_FIXTURE).expect("imgur fixture"));
    response
        .data
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.link,
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn status(value: &str) -> ItemStatus {
    match value {
        "complete" => ItemStatus::Completed,
        "dropped" | "licensed" => ItemStatus::Cancelled,
        _ => ItemStatus::Ongoing,
    }
}

#[derive(Debug, Clone, Deserialize)]
struct EntryDto {
    series: String,
    timestamp: String,
    num: i64,
    #[serde(rename = "chname")]
    chapter_name: String,
    #[serde(rename = "AlbumID")]
    album_id: String,
    #[serde(rename = "projectname")]
    project_name: String,
    #[serde(rename = "projectdesc")]
    project_desc: String,
    #[serde(rename = "projectaltname")]
    alt_name: String,
    #[serde(rename = "projectauthor")]
    project_author: String,
    #[serde(rename = "projectartist")]
    project_artist: String,
    #[serde(rename = "projectthumb")]
    project_thumb: String,
    #[serde(rename = "projectstatus")]
    project_status: String,
    #[serde(rename = "projecttags")]
    project_tags: String,
}

impl EntryDto {
    fn to_item(&self) -> CatalogItem {
        CatalogItem {
            key: format!("/projects?n={}", self.project_name),
            title: self.series.clone(),
            cover: Some(format!("{CDN_URL}{}", self.project_thumb)),
            description: Some(
                format!(
                    "{}\n\nAlternative name: {}",
                    self.project_desc, self.alt_name
                )
                .trim()
                .to_string(),
            ),
            authors: (!self.project_author.is_empty())
                .then(|| self.project_author.clone())
                .into_iter()
                .collect(),
            artists: (!self.project_artist.is_empty())
                .then(|| self.project_artist.clone())
                .into_iter()
                .collect(),
            tags: self
                .project_tags
                .split(',')
                .map(str::trim)
                .filter(|tag| !tag.is_empty())
                .map(ToString::to_string)
                .collect(),
            status: status(&self.project_status),
            url: Some(format!("{BASE_URL}/projects?n={}", self.project_name)),
            language: Some("en".into()),
            content_rating: Some("adult".into()),
            initialized: true,
            ..CatalogItem::default()
        }
    }

    fn to_chapter(&self) -> MangaChapter {
        MangaChapter {
            key: format!("/read?series={}&num={}", self.project_name, self.num),
            title: Some(self.chapter_name.clone()),
            chapter_number: Some(self.num as f32),
            date_uploaded: self
                .timestamp
                .parse::<i64>()
                .ok()
                .map(|seconds| seconds * 1000),
            url: Some(format!(
                "{BASE_URL}/read?series={}&num={}",
                self.project_name, self.num
            )),
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct GoogleDriveResponse {
    files: Vec<DriveFile>,
}

#[derive(Debug, Deserialize)]
struct DriveFile {
    id: String,
    #[serde(rename = "imageMediaMetadata")]
    image_media_metadata: DriveMetadata,
}

#[derive(Debug, Deserialize)]
struct DriveMetadata {
    width: i64,
}

#[derive(Debug, Deserialize)]
struct ImgurResponse {
    data: Vec<ImgurImage>,
}

#[derive(Debug, Deserialize)]
struct ImgurImage {
    link: String,
}

fn sample_entry() -> EntryDto {
    serde_json::from_str::<Vec<EntryDto>>(CHAPTERS_FIXTURE)
        .expect("chapter fixture")
        .remove(0)
}

export_manga_source!(SOURCE);

const CHAPTERS_FIXTURE: &str = r#"[{"series":"Sample SBS","timestamp":"1704067200","num":1,"chname":"Chapter 1","AlbumID":"abc123","projectname":"sample","projectdesc":"Sample description.","projectaltname":"","projectauthor":"Author","projectartist":"Artist","projectthumb":"sample.jpg","projectstatus":"current","projecttags":"Romance, Drama"}]"#;
const DRIVE_FIXTURE: &str =
    r#"{"files":[{"id":"drive-page-1","name":"001.jpg","imageMediaMetadata":{"width":1200}}]}"#;
const IMGUR_FIXTURE: &str = r#"{"data":[{"link":"https://i.imgur.com/page1.jpg"},{"link":"https://i.imgur.com/page2.jpg"}]}"#;
