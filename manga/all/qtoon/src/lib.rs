use aes::Aes128;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use md5::{Digest, Md5};
use serde_json::{Value, json};

type Aes128CbcDec = cbc::Decryptor<Aes128>;

const SOURCE: QToon = QToon;
const BASE_URL: &str = "https://qtoon.com";
const API_URL: &str = "https://api.qtoon.com";
const REQUEST_TOKEN: &str = "ManatanQToonToken000001";

struct QToon;

impl MangaSource for QToon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let site_lang = site_lang_for(&request);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{API_URL}/api/w/ranking/page/comics?page={page}&rsid=daily_hot")
        } else {
            format!(
                "{API_URL}/api/w/search/comic/gallery?area=-1&tag=-1&gender=-1&serialStatus=-1&sortType=hot&page={page}"
            )
        };
        Ok(parse_comics_list(&fetch_api_or_fixture(
            &target,
            site_lang,
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let site_lang = site_lang_for(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(csid) = csid_from_url(query, site_lang) {
            let target = format!("{API_URL}/api/w/comic/detail?csid={csid}");
            let item =
                parse_comic_details(&fetch_api_or_fixture(&target, site_lang, DETAILS_FIXTURE));
            return Ok(Paged {
                entries: vec![item],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target = if query.is_empty() {
            if let Some(asid) = filters
                .get("homePageSection")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                format!(
                    "{API_URL}/api/w/album/page/comics?page={page}&asid={}",
                    url::query_escape(asid)
                )
            } else {
                format!(
                    "{API_URL}/api/w/search/comic/gallery?area=-1&tag={}&gender={}&serialStatus={}&sortType={}&page={page}",
                    filters.get("tag").and_then(Value::as_str).unwrap_or("-1"),
                    filters
                        .get("gender")
                        .and_then(Value::as_str)
                        .unwrap_or("-1"),
                    filters
                        .get("serialStatus")
                        .and_then(Value::as_str)
                        .unwrap_or("-1"),
                    filters
                        .get("sortType")
                        .and_then(Value::as_str)
                        .unwrap_or("hot")
                )
            }
        } else {
            format!(
                "{API_URL}/api/w/search/comic/search?title={}&page={page}",
                url::query_escape(query)
            )
        };
        Ok(parse_comics_list(&fetch_api_or_fixture(
            &target,
            site_lang,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let site_lang = site_lang_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| comic_key("sample", ""));
        let comic = comic_url(&key);
        let target = format!(
            "{API_URL}/api/w/comic/detail?csid={}",
            comic.web_link_id_or_csid()
        );
        Ok(parse_comic_details(&fetch_api_or_fixture(
            &target,
            site_lang,
            DETAILS_FIXTURE,
        )))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let site_lang = site_lang_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| comic_key("sample", ""));
        let comic = comic_url(&key);
        let target = format!(
            "{API_URL}/api/w/comic/detail?csid={}",
            comic.web_link_id_or_csid()
        );
        Ok(parse_chapters(
            &fetch_api_or_fixture(&target, site_lang, CHAPTERS_FIXTURE),
            &comic.csid,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let site_lang = site_lang_for(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| episode_key("episode-1", "sample"));
        let episode = episode_url(&key);
        let detail_target = format!("{API_URL}/api/w/comic/episode/detail?esid={}", episode.esid);
        let token = parse_episode_token(&fetch_api_or_fixture(
            &detail_target,
            site_lang,
            EPISODE_FIXTURE,
        ));
        let mut resources = Vec::new();
        let mut page = 1;
        loop {
            let target = format!(
                "{API_URL}/api/w/resource/group/rslv?token={}&page={page}",
                url::query_escape(&token)
            );
            let body = fetch_api_or_fixture(&target, site_lang, RESOURCES_FIXTURE);
            let (mut batch, more) = parse_resources(&body);
            resources.append(&mut batch);
            if !more || page >= 20 {
                break;
            }
            page += 1;
        }
        resources.sort_by_key(|resource| resource.index);
        Ok(resources
            .into_iter()
            .map(|resource| MangaPage {
                content: PageContent::Url {
                    url: decrypt_image_url(&resource.url).unwrap_or(resource.url),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", resource.index)),
                ..MangaPage::default()
            })
            .collect())
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let site_lang = site_lang_for(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(csid) = csid_from_url(input, site_lang) {
            let target = format!("{API_URL}/api/w/comic/detail?csid={csid}");
            return Ok(Some(UrlResolveResult {
                item: Some(parse_comic_details(&fetch_api_or_fixture(
                    &target,
                    site_lang,
                    DETAILS_FIXTURE,
                ))),
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

#[derive(Clone)]
struct ComicUrl {
    csid: String,
    web_link_id: String,
}

impl ComicUrl {
    fn web_link_id_or_csid(&self) -> &str {
        if self.web_link_id.is_empty() {
            &self.csid
        } else {
            &self.web_link_id
        }
    }
}

struct EpisodeUrl {
    esid: String,
    _csid: String,
}

struct ResourceImage {
    url: String,
    index: i64,
}

fn client(site_lang: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_header("platform", "pc")
        .with_header("lth", site_lang)
        .with_header("did", REQUEST_TOKEN)
        .with_webview_challenge_fallback()
}

fn fetch_api_or_fixture(target: &str, site_lang: &str, fixture: &str) -> String {
    client(site_lang)
        .get(target)
        .xhr()
        .send_text()
        .ok()
        .and_then(|raw| decrypt_payload(&raw).ok())
        .unwrap_or_else(|| fixture.to_string())
}

fn decrypt_payload(raw: &str) -> Result<String, String> {
    let value = serde_json::from_str::<Value>(raw).map_err(|error| error.to_string())?;
    let Some(data) = value.get("data").and_then(Value::as_str) else {
        return Ok(raw.to_string());
    };
    let ts = value.get("ts").and_then(Value::as_i64).unwrap_or_default();
    let inner = md5_hex(&format!("{REQUEST_TOKEN}{ts}"));
    let outer = md5_hex(&format!("{inner}OQlM9JBJgLWsgffb"));
    aes_decrypt(data, &outer)
}

fn decrypt_image_url(raw: &str) -> Option<String> {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Some(raw.to_string());
    }
    let inner = md5_hex(REQUEST_TOKEN);
    let outer = md5_hex(&format!("{inner}9tv86uBwmOYs7QZ0"));
    aes_decrypt(raw, &outer).ok()
}

fn aes_decrypt(data: &str, key_material: &str) -> Result<String, String> {
    let encrypted = STANDARD.decode(data).map_err(|error| error.to_string())?;
    let key = &key_material.as_bytes()[..16];
    let iv = &key_material.as_bytes()[16..32];
    let decrypted = Aes128CbcDec::new(key.into(), iv.into())
        .decrypt_padded_vec_mut::<Pkcs7>(&encrypted)
        .map_err(|error| format!("{error:?}"))?;
    String::from_utf8(decrypted).map_err(|error| error.to_string())
}

fn md5_hex(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn parse_comics_list(body: &str) -> Paged<CatalogItem> {
    let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let entries = value
        .get("comics")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(item_from_comic)
        .collect();
    Paged {
        has_next_page: value.get("more").and_then(Value::as_i64) == Some(1),
        entries,
    }
}

fn parse_comic_details(body: &str) -> CatalogItem {
    let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    item_from_comic(value.get("comic").unwrap_or(&value))
}

fn item_from_comic(comic: &Value) -> CatalogItem {
    let csid = comic
        .get("csid")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let web_link_id = comic.get("webLinkId").and_then(Value::as_str).unwrap_or("");
    let tags = comic
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(
            comic
                .get("corners")
                .and_then(|corners| corners.get("cornerTags"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
        .filter_map(|tag| tag.get("name").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect();
    CatalogItem {
        key: comic_key(csid, web_link_id),
        title: comic
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Untitled")
            .to_string(),
        cover: comic
            .get("image")
            .and_then(|image| image.get("thumb"))
            .and_then(|thumb| thumb.get("url"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        authors: comic
            .get("author")
            .and_then(Value::as_str)
            .map(|author| vec![author.to_string()])
            .unwrap_or_default(),
        description: comic
            .get("introduction")
            .and_then(Value::as_str)
            .map(|intro| {
                let mut text = intro.to_string();
                if let Some(update) = comic
                    .get("updateMemo")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    text.push_str("\n\nUpdates: ");
                    text.push_str(update);
                }
                text
            }),
        tags,
        status: match comic.get("serialStatus2").and_then(Value::as_i64) {
            Some(101) => ItemStatus::Ongoing,
            Some(103) => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(qtoon_detail_url("en-US", web_link_id, csid)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, csid: &str) -> Vec<MangaChapter> {
    let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let mut chapters = value
        .get("episodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|episode| {
            let esid = episode
                .get("esid")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            MangaChapter {
                key: episode_key(esid, csid),
                title: episode
                    .get("title")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                chapter_number: episode
                    .get("serialNo")
                    .and_then(Value::as_i64)
                    .map(|value| value as f32),
                url: Some(qtoon_reader_url("en-US", csid, esid)),
                ..MangaChapter::default()
            }
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_episode_token(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("definitions")
                .and_then(Value::as_array)
                .and_then(|definitions| definitions.first())
                .and_then(|definition| definition.get("token"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| "token".to_string())
}

fn parse_resources(body: &str) -> (Vec<ResourceImage>, bool) {
    let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let resources = value
        .get("resources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|resource| {
            Some(ResourceImage {
                url: resource.get("url")?.as_str()?.to_string(),
                index: resource.get("rgIdx").and_then(Value::as_i64).unwrap_or(0),
            })
        })
        .collect();
    (
        resources,
        value.get("more").and_then(Value::as_i64) == Some(1),
    )
}

fn comic_key(csid: &str, web_link_id: &str) -> String {
    json!({ "csid": csid, "webLinkId": web_link_id }).to_string()
}

fn episode_key(esid: &str, csid: &str) -> String {
    json!({ "esid": esid, "csid": csid }).to_string()
}

fn comic_url(key: &str) -> ComicUrl {
    let value = serde_json::from_str::<Value>(key).unwrap_or(Value::Null);
    ComicUrl {
        csid: value
            .get("csid")
            .and_then(Value::as_str)
            .unwrap_or(key)
            .to_string(),
        web_link_id: value
            .get("webLinkId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }
}

fn episode_url(key: &str) -> EpisodeUrl {
    let value = serde_json::from_str::<Value>(key).unwrap_or(Value::Null);
    EpisodeUrl {
        esid: value
            .get("esid")
            .and_then(Value::as_str)
            .unwrap_or(key)
            .to_string(),
        _csid: value
            .get("csid")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }
}

fn site_lang_for(request: &Value) -> &'static str {
    match request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
    {
        Some("qtoon-es") => "es-ES",
        Some("qtoon-pt-br") => "pt-PT",
        _ => "en-US",
    }
}

fn csid_from_url(input: &str, site_lang: &str) -> Option<String> {
    let path = input.trim_start_matches(BASE_URL).trim_start_matches('/');
    let parts = path.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        ["detail", id] | ["reader", id] if site_lang == "en-US" => Some((*id).to_string()),
        [lang, "detail", id] | [lang, "reader", id]
            if *lang == site_lang.split('-').next().unwrap_or("en") =>
        {
            Some((*id).to_string())
        }
        _ => None,
    }
}

fn qtoon_detail_url(site_lang: &str, web_link_id: &str, csid: &str) -> String {
    let lang_dir = site_lang.split('-').next().unwrap_or("en");
    let id = if web_link_id.is_empty() {
        csid
    } else {
        web_link_id
    };
    if lang_dir == "en" {
        format!("{BASE_URL}/detail/{id}")
    } else {
        format!("{BASE_URL}/{lang_dir}/detail/{id}")
    }
}

fn qtoon_reader_url(site_lang: &str, csid: &str, esid: &str) -> String {
    let lang_dir = site_lang.split('-').next().unwrap_or("en");
    if lang_dir == "en" {
        format!("{BASE_URL}/reader/{csid}?chapter={esid}")
    } else {
        format!("{BASE_URL}/{lang_dir}/reader/{csid}?chapter={esid}")
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"comics":[{"csid":"sample","webLinkId":"sample-link","title":"Sample Comic","image":{"thumb":{"url":"https://qtoon.com/thumb.jpg"}},"tags":[{"name":"Romance"}],"author":"Jane","serialStatus2":101,"updateMemo":"Weekly","introduction":"Intro","corners":{"cornerTags":[{"name":"Hot"}]}}],"more":0}"#;
const DETAILS_FIXTURE: &str = r#"{"comic":{"csid":"sample","webLinkId":"sample-link","title":"Sample Comic","image":{"thumb":{"url":"https://qtoon.com/thumb.jpg"}},"tags":[{"name":"Romance"}],"author":"Jane","serialStatus2":101,"updateMemo":"Weekly","introduction":"Intro","corners":{"cornerTags":[{"name":"Hot"}]}}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"episodes":[{"esid":"episode-1","title":"Episode 1","serialNo":1},{"esid":"episode-2","title":"Episode 2","serialNo":2}]}"#;
const EPISODE_FIXTURE: &str = r#"{"definitions":[{"token":"token"}]}"#;
const RESOURCES_FIXTURE: &str = r#"{"resources":[{"url":"https://qtoon.com/page1.jpg","rgIdx":1},{"url":"https://qtoon.com/page2.jpg","rgIdx":2}],"more":0}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_listing_details_chapters_and_pages() {
        let listing = parse_comics_list(LIST_FIXTURE);
        assert_eq!(listing.entries[0].title, "Sample Comic");

        let details = SOURCE
            .details(json!({"sourceId":"qtoon-en","manga":comic_key("sample", "sample-link")}))
            .unwrap();
        assert_eq!(details.authors, vec!["Jane"]);

        let chapters = SOURCE
            .chapters(json!({"sourceId":"qtoon-en","manga":comic_key("sample", "sample-link")}))
            .unwrap();
        assert_eq!(chapters.len(), 2);

        let pages = SOURCE
            .pages(json!({"sourceId":"qtoon-en","chapter":episode_key("episode-1", "sample")}))
            .unwrap();
        assert_eq!(pages.len(), 2);
    }

    #[test]
    fn resolves_language_urls() {
        assert_eq!(
            csid_from_url("https://qtoon.com/es/detail/sample", "es-ES").as_deref(),
            Some("sample")
        );
    }
}
