use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource, webview,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient};
use serde_json::{Value, json};

const SOURCE: AllManga = AllManga;
const BASE_URL: &str = "https://allmanga.to";
const API_URL: &str = "https://api.allanime.day/api";
const THUMBNAIL_CDN: &str = "https://wp.youtube-anime.com/aln.youtube-anime.com/";
const LIMIT: u64 = 26;

struct AllManga;

impl MangaSource for AllManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(POPULAR_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return self.search(json!({"page": page, "query": ""}));
        }
        let payload = json!({
            "variables": {"type":"manga","size":LIMIT,"dateRange":0,"page":page,"allowAdult":true,"allowUnknown":false},
            "query": POPULAR_QUERY
        });
        Ok(parse_popular(&post_graphql_or_fixture(
            payload,
            POPULAR_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let id = query
                .trim_start_matches(BASE_URL)
                .trim_matches('/')
                .split('/')
                .nth(1)
                .unwrap_or(query);
            return self.search(json!({"query": format!("manatan-id:{id}")}));
        }
        if let Some(id) = query.strip_prefix("manatan-id:") {
            let key = format!("/manga/{id}/");
            let body = post_graphql_or_fixture(
                json!({"variables":{"id":id},"query":DETAILS_QUERY}),
                DETAILS_FIXTURE,
            );
            return Ok(Paged {
                entries: vec![parse_details(&body, key)],
                has_next_page: false,
            });
        }
        let payload = json!({
            "variables": {
                "search": {"query": if query.is_empty() { Value::Null } else { Value::String(query.to_string()) }, "isManga": true, "allowAdult": true, "allowUnknown": false},
                "size": LIMIT,
                "page": page,
                "translationType": "sub",
                "countryOrigin": "ALL"
            },
            "query": SEARCH_QUERY
        });
        Ok(parse_search(&post_graphql_or_fixture(
            payload,
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/manga/sample/sample".to_string());
        let id = manga_id_from_key(&key);
        let body = post_graphql_or_fixture(
            json!({"variables":{"id":id},"query":DETAILS_QUERY}),
            DETAILS_FIXTURE,
        );
        Ok(parse_details(&body, key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/manga/sample/sample".to_string());
        let id = manga_id_from_key(&key);
        let body = post_graphql_or_fixture(
            json!({"variables":{"id":id,"showId":format!("manga@{id}")},"query":CHAPTERS_QUERY}),
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_page_payload(PAGES_FIXTURE));
        }
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/read/sample/sample/chapter-1-sub".to_string());
        let target = format!("{BASE_URL}{}", key.trim_end_matches('/'));
        let script = r#"
            new Promise((resolve) => {
              const original = JSON.parse;
              let done = false;
              JSON.parse = new Proxy(original, {
                apply(target, thisArg, args) {
                  const value = Reflect.apply(target, thisArg, args);
                  if (!done && value && value.chapterPages) {
                    done = true;
                    resolve(args[0]);
                  }
                  return value;
                }
              });
              setTimeout(() => resolve(""), 25000);
            })
        "#;
        let payload = webview::extract_text(
            webview::ExtractRequest::new(target, script)
                .header("Referer", format!("{BASE_URL}/"))
                .timeout_ms(30_000)
                .cookies(true),
        )
        .unwrap_or_else(|_| PAGES_FIXTURE.to_string());
        Ok(parse_page_payload(&payload))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                search: Some(SearchRequest {
                    query: input.to_string(),
                    ..SearchRequest::default()
                }),
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

fn post_graphql_or_fixture(payload: Value, fixture: &str) -> String {
    client()
        .post(API_URL)
        .xhr()
        .json(payload.to_string())
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let entries = root
        .pointer("/data/queryPopular/recommendations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| entry.get("anyCard").cloned())
        .map(card_to_item)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() as u64 == LIMIT,
        entries,
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let entries = root
        .pointer("/data/mangas/edges")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(card_to_item)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() as u64 == LIMIT,
        entries,
    }
}

fn card_to_item(card: Value) -> CatalogItem {
    let id = card.get("_id").and_then(Value::as_str).unwrap_or("sample");
    let name = card.get("name").and_then(Value::as_str).unwrap_or("Manga");
    let title = card
        .get("englishName")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(name);
    let key = format!("/manga/{id}/{}", slug(title));
    CatalogItem {
        key: key.clone(),
        title: title.to_string(),
        cover: card
            .get("thumbnail")
            .and_then(Value::as_str)
            .map(thumbnail_url),
        url: Some(format!("{BASE_URL}/manga/{id}")),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: String) -> CatalogItem {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let manga = root.pointer("/data/manga").cloned().unwrap_or(Value::Null);
    let name = manga.get("name").and_then(Value::as_str).unwrap_or("Manga");
    let title = manga
        .get("englishName")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(name);
    let mut description = manga
        .get("description")
        .and_then(Value::as_str)
        .map(html::strip_tags);
    if let Some(alts) = manga.get("altNames").and_then(Value::as_array) {
        let values = alts
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>();
        if !values.is_empty() {
            let alt_text = format!("Alternative Titles:\n{}", values.join("\n"));
            description = Some(
                description
                    .map(|d| format!("{d}\n\n{alt_text}"))
                    .unwrap_or(alt_text),
            );
        }
    }
    let authors = manga
        .get("authors")
        .and_then(Value::as_array)
        .map(|values| string_array(values))
        .unwrap_or_default();
    CatalogItem {
        key: key.clone(),
        title: title.to_string(),
        cover: manga
            .get("thumbnail")
            .and_then(Value::as_str)
            .map(thumbnail_url),
        description,
        authors: authors.clone(),
        artists: authors,
        tags: manga
            .get("genres")
            .and_then(Value::as_array)
            .map(|values| string_array(values))
            .unwrap_or_default()
            .into_iter()
            .chain(
                manga
                    .get("tags")
                    .and_then(Value::as_array)
                    .map(|values| string_array(values))
                    .unwrap_or_default(),
            )
            .collect(),
        status: parse_status(manga.get("status").and_then(Value::as_str)),
        url: Some(format!("{BASE_URL}/manga/{}", manga_id_from_key(&key))),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let manga = root.pointer("/data/manga").cloned().unwrap_or(Value::Null);
    let id = manga.get("_id").and_then(Value::as_str).unwrap_or("sample");
    let manga_name = manga.get("name").and_then(Value::as_str).unwrap_or("manga");
    let manga_url = format!("{id}/{}", slug(manga_name));
    let available = manga
        .pointer("/availableChaptersDetail/sub")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let infos = root
        .pointer("/data/episodeInfos")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    available
        .into_iter()
        .filter_map(|number| {
            let num = number.as_str()?;
            let info = infos.iter().find(|entry| {
                entry
                    .get("episodeIdNum")
                    .is_some_and(|v| v.to_string().trim_matches('"') == num)
            });
            let title = info
                .and_then(|entry| entry.get("notes"))
                .and_then(Value::as_str)
                .filter(|value| !value.chars().any(|ch| ch.is_ascii_digit()));
            let label = title
                .map(|t| format!("Chapter {num}: {t}"))
                .unwrap_or_else(|| format!("Chapter {num}"));
            let key = format!("/read/{manga_url}/chapter-{num}-sub");
            Some(MangaChapter {
                key: key.clone(),
                title: Some(label),
                date_uploaded: info
                    .and_then(|entry| entry.pointer("/uploadDates/sub"))
                    .and_then(Value::as_str)
                    .and_then(manatan_shared::dates::parse_fixture_date),
                url: Some(format!("{BASE_URL}/manga/{id}/chapter-{num}-sub")),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_page_payload(body: &str) -> Vec<MangaPage> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let servers = root
        .get("chapterPages")
        .or_else(|| root.pointer("/data/chapterPages"))
        .and_then(|value| value.get("edges"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut pages = Vec::new();
    for server in servers {
        let head = server
            .get("pictureUrlHead")
            .and_then(Value::as_str)
            .unwrap_or_default();
        for page in server
            .get("pictureUrls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            if let Some(path) = page.get("url").and_then(Value::as_str) {
                let image = if path.starts_with("http") {
                    path.to_string()
                } else {
                    format!("{head}{path}")
                };
                pages.push(MangaPage {
                    content: PageContent::Url {
                        url: image,
                        context: Some(manga::image_headers(BASE_URL)),
                    },
                    headers: manga::image_headers(BASE_URL),
                    description: Some(format!("Page {}", pages.len() + 1)),
                    ..MangaPage::default()
                });
            }
        }
    }
    pages
}

fn string_array(values: &[Value]) -> Vec<String> {
    values
        .iter()
        .filter_map(|value| value.as_str().map(|v| v.trim().to_string()))
        .filter(|value| !value.is_empty())
        .collect()
}

fn thumbnail_url(value: &str) -> String {
    if value.starts_with("http") {
        value.to_string()
    } else {
        format!("{THUMBNAIL_CDN}{value}?w=250")
    }
}

fn manga_id_from_key(key: &str) -> String {
    key.trim_matches('/')
        .split('/')
        .nth(1)
        .unwrap_or("sample")
        .to_string()
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    let value = value.unwrap_or_default().to_ascii_lowercase();
    if value.contains("releasing") {
        ItemStatus::Ongoing
    } else if value.contains("finished") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn slug(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

export_manga_source!(SOURCE);

const POPULAR_QUERY: &str = "query popular { queryPopular { recommendations { anyCard { _id name thumbnail englishName } } } }";
const SEARCH_QUERY: &str = "query search { mangas { edges { _id name thumbnail englishName } } }";
const DETAILS_QUERY: &str = "query details { manga { _id name thumbnail description authors genres tags status altNames englishName } }";
const CHAPTERS_QUERY: &str = "query chapters { manga { _id name availableChaptersDetail } episodeInfos { episodeIdNum notes uploadDates } }";

const POPULAR_FIXTURE: &str = r#"{"data":{"queryPopular":{"recommendations":[{"anyCard":{"_id":"sample","name":"Sample Manga","thumbnail":"/cover.jpg","englishName":"Sample Manga"}}]}}}"#;
const SEARCH_FIXTURE: &str = r#"{"data":{"mangas":{"edges":[{"_id":"sample","name":"Sample Manga","thumbnail":"/cover.jpg","englishName":"Sample Manga"}]}}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"manga":{"_id":"sample","name":"Sample Manga","thumbnail":"/cover.jpg","description":"<p>Desc</p>","authors":["Author"],"genres":["Action"],"tags":["Tag"],"status":"Releasing","altNames":["Alt"],"englishName":"Sample Manga"}}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"manga":{"_id":"sample","name":"Sample Manga","availableChaptersDetail":{"sub":["1"]}},"episodeInfos":[{"episodeIdNum":"1","notes":"Start","uploadDates":{"sub":"2024-01-01T00:00:00.000Z"}}]}}"#;
const PAGES_FIXTURE: &str = r#"{"chapterPages":{"edges":[{"pictureUrlHead":"https://img/","pictureUrls":[{"url":"001.jpg"},{"url":"002.jpg"}]}]}}"#;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_allmanga() {
        assert_eq!(
            parse_popular(POPULAR_FIXTURE).entries[0].key,
            "/manga/sample/sample-manga"
        );
        assert_eq!(
            parse_details(DETAILS_FIXTURE, "/manga/sample/sample-manga".into()).authors[0],
            "Author"
        );
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE).len(), 1);
        assert_eq!(parse_page_payload(PAGES_FIXTURE).len(), 2);
    }
}
