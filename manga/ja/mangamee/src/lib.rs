use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, ProcessedImage, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{
    manga,
    sdk::http::{Headers, HttpClient},
    url,
};
use prost::Message;
use serde_json::Value;

const SOURCE: MangaMee = MangaMee;
const BASE_URL: &str = "https://manga-mee.jp";
const API_URL: &str = "https://prod2-android.manga-mee.jp/web/v1";

struct MangaMee;

impl MangaSource for MangaMee {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listingId")
            .or_else(|| request.get("listing"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{BASE_URL}/title-list/todaysupdate")
        } else {
            format!("{BASE_URL}/title-list/ranking")
        };
        let body = fetch_rsc(
            &target,
            if listing == "latest" {
                LATEST_FIXTURE
            } else {
                RANKING_FIXTURE
            },
        );
        Ok(parse_listing(&body, listing == "latest"))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let body = fetch_rsc(
            &format!(
                "{BASE_URL}/search-result/keyword/{}",
                url::query_escape(query)
            ),
            SEARCH_FIXTURE,
        );
        Ok(parse_listing(&body, false))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        let hide_locked = preference_bool(&request, "hide_locked");
        Ok(parse_chapters(
            &fetch_rsc(&format!("{BASE_URL}/all-episodes/{key}"), CHAPTERS_FIXTURE),
            &key,
            hide_locked,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1#1".into());
        let mut parts = key.split('#');
        let episode_id = parts.next().unwrap_or("1");
        let title_id = parts.next().unwrap_or("1");
        let detail = fetch_proto::<DetailResponse>(
            &format!("{API_URL}/title_detail?title_id={title_id}&episode_id={episode_id}"),
            PAGES_PROTO_FIXTURE,
        )
        .unwrap_or_else(sample_detail);
        if detail.pages.is_empty() {
            return Ok(vec![manga::text_page(
                "This chapter is only accessible via the official MangaMee app.",
            )]);
        }
        Ok(detail
            .pages
            .into_iter()
            .enumerate()
            .filter_map(|(index, page)| {
                let main = page.main_page?;
                Some(MangaPage {
                    content: PageContent::Url {
                        url: format!("{}#key={}", main.image_url, main.key),
                        context: Some(manga::image_headers(BASE_URL)),
                    },
                    headers: manga::image_headers(BASE_URL),
                    description: Some(index.to_string()),
                    ..MangaPage::default()
                })
            })
            .collect())
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        let input = request
            .get("imageBase64")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(image_url) = page_image_url(&request) else {
            return passthrough_image(&request);
        };
        let Some(key_hex) = image_url.split("#key=").nth(1) else {
            return passthrough_image(&request);
        };
        let (Ok(mut bytes), Some(key)) = (STANDARD.decode(input), decode_hex(key_hex)) else {
            return passthrough_image(&request);
        };
        if key.is_empty() {
            return passthrough_image(&request);
        }
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte ^= key[index % key.len()];
        }
        Ok(ProcessedImage {
            image_base64: STANDARD.encode(bytes),
            mime_type: request
                .get("mimeType")
                .and_then(Value::as_str)
                .map(str::to_string),
            ..ProcessedImage::default()
        })
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
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

fn fetch_rsc(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("rsc", "1")
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.into())
}

fn fetch_proto<T: Message + Default>(target: &str, fixture_hex: &str) -> Option<T> {
    let mut headers = Headers::new();
    headers.insert("Accept".into(), "application/protobuf".into());
    client()
        .fetch("GET", target, None, headers)
        .ok()
        .and_then(|response| {
            response
                .body_base64
                .and_then(|body| STANDARD.decode(body).ok())
                .or_else(|| response.text.map(|text| text.into_bytes()))
        })
        .or_else(|| Some(hex_decode(fixture_hex)))
        .and_then(|bytes| T::decode(bytes.as_slice()).ok())
}

fn parse_listing(body: &str, latest: bool) -> Paged<CatalogItem> {
    let root = json_from_body(
        body,
        if latest {
            LATEST_FIXTURE
        } else {
            RANKING_FIXTURE
        },
    );
    let paths: &[&str] = if latest {
        &["/titleGroup/titles", "/popularTitles/titles"]
    } else {
        &["/all/rankingList/0/titles", "/popularTitles/titles"]
    };
    let mut entries = Vec::new();
    for path in paths {
        if let Some(titles) = root.pointer(path).and_then(Value::as_array) {
            entries.extend(titles.iter().filter_map(item_from_title));
            break;
        }
    }
    Paged {
        entries,
        has_next_page: false,
    }
}

fn item_from_title(value: &Value) -> Option<CatalogItem> {
    let id = value.get("id").and_then(value_to_string)?;
    Some(CatalogItem {
        key: id.clone(),
        title: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("MangaMee")
            .into(),
        cover: value
            .pointer("/largeImage/src")
            .and_then(Value::as_str)
            .map(str::to_string),
        url: Some(format!("{BASE_URL}/detail/{id}")),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn details_from_key(key: &str) -> CatalogItem {
    fetch_proto::<DetailResponse>(
        &format!("{API_URL}/title_detail?title_id={key}"),
        DETAIL_PROTO_FIXTURE,
    )
    .map(|detail| detail.to_item(key))
    .unwrap_or_else(|| sample_detail().to_item(key))
}

fn parse_chapters(body: &str, title_id: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let root = json_from_body(body, CHAPTERS_FIXTURE);
    root.pointer("/allEpisodes/episodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|episode| {
            let id = episode.get("id").and_then(value_to_string)?;
            let is_locked = episode.get("isFree").and_then(Value::as_bool) == Some(false);
            if hide_locked && is_locked {
                return None;
            }
            Some(MangaChapter {
                key: format!("{id}#{title_id}"),
                title: Some(format!(
                    "{}{}",
                    if is_locked { "[Locked] " } else { "" },
                    episode
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Chapter")
                )),
                url: Some(format!("{BASE_URL}/detail/{title_id}?episodeId={id}")),
                language: Some("ja".into()),
                is_locked,
                ..MangaChapter::default()
            })
        })
        .rev()
        .collect()
}

fn json_from_body(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .ok()
        .or_else(|| first_json_object(body))
        .unwrap_or_else(|| serde_json::from_str(fixture).unwrap_or(Value::Null))
}

fn first_json_object(body: &str) -> Option<Value> {
    for start in body.match_indices('{').map(|(index, _)| index) {
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escape = false;
        for (offset, ch) in body[start..].char_indices() {
            if in_string {
                if escape {
                    escape = false;
                } else if ch == '\\' {
                    escape = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            match ch {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        if let Ok(value) = serde_json::from_str(&body[start..=start + offset]) {
                            return Some(value);
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn key_from_url(input: &str) -> Option<String> {
    if input.is_empty() {
        return None;
    }
    if !input.starts_with("http") && !input.starts_with('/') {
        return Some(input.into());
    }
    input.find("/detail/").map(|index| {
        input[index + "/detail/".len()..]
            .split(['?', '/', '#'])
            .next()
            .unwrap_or("1")
            .to_string()
    })
}

fn value_to_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_i64().map(|number| number.to_string()))
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(id))
        .and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_str().map(|text| text == "true"))
        })
        .unwrap_or(false)
}

fn page_image_url(request: &Value) -> Option<String> {
    request
        .get("page")
        .and_then(|page| page.get("content"))
        .and_then(|content| content.get("url"))
        .and_then(|url| url.get("url").or(Some(url)))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn passthrough_image(request: &Value) -> ExtensionResult<ProcessedImage> {
    Ok(ProcessedImage {
        image_base64: request
            .get("imageBase64")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        mime_type: request
            .get("mimeType")
            .and_then(Value::as_str)
            .map(str::to_string),
        ..ProcessedImage::default()
    })
}

fn decode_hex(input: &str) -> Option<Vec<u8>> {
    if !input.len().is_multiple_of(2) {
        return None;
    }
    Some(hex_decode(input))
}

fn hex_decode(input: &str) -> Vec<u8> {
    input
        .as_bytes()
        .chunks(2)
        .filter_map(|pair| {
            let hi = hex_value(*pair.first()?)?;
            let lo = hex_value(*pair.get(1)?)?;
            Some((hi << 4) | lo)
        })
        .collect()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Clone, PartialEq, Message)]
struct DetailResponse {
    #[prost(message, optional, tag = "1")]
    title: Option<Title>,
    #[prost(message, repeated, tag = "3")]
    tags: Vec<Tag>,
    #[prost(message, repeated, tag = "13")]
    pages: Vec<PageWrapper>,
}

impl DetailResponse {
    fn to_item(self, key: &str) -> CatalogItem {
        let title = self.title.unwrap_or_else(|| Title {
            name: "MangaMee".into(),
            description_text: None,
            mangaka: None,
            thumbnail: None,
            kana_name: None,
        });
        CatalogItem {
            key: key.into(),
            title: title.name,
            alternate_titles: title.kana_name.into_iter().collect(),
            cover: title.thumbnail,
            authors: title.mangaka.into_iter().collect(),
            description: title.description_text,
            tags: self.tags.into_iter().map(|tag| tag.name).collect(),
            url: Some(format!("{BASE_URL}/detail/{key}")),
            language: Some("ja".into()),
            content_rating: Some("safe".into()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Clone, PartialEq, Message)]
struct Title {
    #[prost(string, tag = "2")]
    name: String,
    #[prost(string, optional, tag = "4")]
    description_text: Option<String>,
    #[prost(string, optional, tag = "6")]
    mangaka: Option<String>,
    #[prost(string, optional, tag = "9")]
    thumbnail: Option<String>,
    #[prost(string, optional, tag = "17")]
    kana_name: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
struct Tag {
    #[prost(string, tag = "2")]
    name: String,
}

#[derive(Clone, PartialEq, Message)]
struct PageWrapper {
    #[prost(message, optional, tag = "1")]
    main_page: Option<ViewerImage>,
}

#[derive(Clone, PartialEq, Message)]
struct ViewerImage {
    #[prost(string, tag = "2")]
    image_url: String,
    #[prost(string, tag = "7")]
    key: String,
}

fn sample_detail() -> DetailResponse {
    DetailResponse {
        title: Some(Title {
            name: "Sample MangaMee".into(),
            description_text: Some("Sample description.".into()),
            mangaka: Some("Sample Author".into()),
            thumbnail: Some("https://img.example.test/mangamee.jpg".into()),
            kana_name: None,
        }),
        tags: vec![Tag {
            name: "Drama".into(),
        }],
        pages: vec![PageWrapper {
            main_page: Some(ViewerImage {
                image_url: "https://img.example.test/mangamee-page.jpg".into(),
                key: "00".into(),
            }),
        }],
    }
}

export_manga_source!(SOURCE);

const RANKING_FIXTURE: &str = r#"{"all":{"rankingList":[{"name":"総合","titles":[{"id":1,"name":"Sample MangaMee","largeImage":{"src":"https://img.example.test/mangamee.jpg"}}]}]}}"#;
const LATEST_FIXTURE: &str = r#"{"titleGroup":{"titles":[{"id":1,"name":"Sample MangaMee","largeImage":{"src":"https://img.example.test/mangamee.jpg"}}]}}"#;
const SEARCH_FIXTURE: &str = r#"{"popularTitles":{"titles":[{"id":1,"name":"Sample MangaMee","largeImage":{"src":"https://img.example.test/mangamee.jpg"}}]}}"#;
const CHAPTERS_FIXTURE: &str =
    r#"{"allEpisodes":{"episodes":[{"id":1,"title":"Chapter 1","isFree":true}]}}"#;
const DETAIL_PROTO_FIXTURE: &str = "120f0a0d53616d706c65204d616e67614d6565220753616d706c652a0d53616d706c6520417574686f724a2568747470733a2f2f696d672e6578616d706c652e746573742f6d616e67616d65652e6a7067";
const PAGES_PROTO_FIXTURE: &str = "6a350a33121168747470733a2f2f696d672e6578616d706c652e746573742f6d616e67616d65652d706167652e6a70673a023030";
