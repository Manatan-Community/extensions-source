use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient};
use serde_json::{Value, json};

const SOURCE: Ganma = Ganma;
const BASE_URL: &str = "https://ganma.jp";
const API_URL: &str = "https://ganma.jp/api/graphql";

struct Ganma;

impl MangaSource for Ganma {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_latest(&graphql_or_fixture(
                "serialMagazinesByDayOfWeek",
                HASH_SERIAL_MAGAZINES_BY_DAY_OF_WEEK,
                json!({"dayOfWeek": "MONDAY", "after": null}),
                LATEST_FIXTURE,
                false,
            )));
        }
        Ok(parse_popular(&graphql_or_fixture(
            "home",
            HASH_HOME,
            json!({}),
            POPULAR_FIXTURE,
            false,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(alias) = alias_from_input(query) {
            return Ok(Paged {
                entries: vec![details_from_alias(&alias)],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            return Ok(parse_search(
                &graphql_or_fixture(
                    "magazinesByKeywordSearch",
                    HASH_MAGAZINES_BY_KEYWORD_SEARCH,
                    json!({"keyword": query, "after": null}),
                    SEARCH_FIXTURE,
                    false,
                ),
                "/data/searchComic",
            ));
        }
        let category = filter_string(&request, "category").unwrap_or("MONDAY");
        if category == "finished" {
            return Ok(parse_search(
                &graphql_or_fixture(
                    "finishedMagazines",
                    HASH_FINISHED_MAGAZINES,
                    json!({"after": null}),
                    FINISHED_FIXTURE,
                    false,
                ),
                "/data/magazinesByCategory/magazines",
            ));
        }
        Ok(parse_latest(&graphql_or_fixture(
            "serialMagazinesByDayOfWeek",
            HASH_SERIAL_MAGAZINES_BY_DAY_OF_WEEK,
            json!({"dayOfWeek": category, "after": null}),
            LATEST_FIXTURE,
            false,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let alias = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(details_from_alias(&alias))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let alias = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_chapters(
            &graphql_or_fixture(
                "storyInfoList",
                HASH_STORY_INFO_LIST,
                json!({"magazineIdOrAlias": alias, "first": 9999, "after": null}),
                CHAPTERS_FIXTURE,
                false,
            ),
            &alias,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/story-1".to_string());
        let mut parts = key.split('/');
        let alias = parts.next().unwrap_or("sample");
        let story_id = parts.next().unwrap_or("story-1");
        Ok(parse_pages(&graphql_or_fixture(
            "magazineStoryForReader",
            HASH_MAGAZINE_STORY_FOR_READER,
            json!({"magazineIdOrAlias": alias, "storyId": story_id}),
            PAGES_FIXTURE,
            true,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(alias) = alias_from_input(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_alias(&alias)),
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

fn client(use_app_reader_ua: bool) -> HttpClient {
    let ua = if use_app_reader_ua {
        "GanmaReader/10.7.0 Android"
    } else {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36"
    };
    HttpClient::browser()
        .with_header("User-Agent", ua)
        .with_header("X-From", "https://ganma.jp/web")
        .with_referer(format!("{BASE_URL}/web/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn graphql_or_fixture(
    operation_name: &str,
    hash: &str,
    variables: Value,
    fixture: &str,
    use_app_reader_ua: bool,
) -> String {
    let body = json!({
        "operationName": operation_name,
        "variables": variables,
        "extensions": {
            "persistedQuery": {
                "version": 1,
                "sha256Hash": hash
            }
        }
    });
    client(use_app_reader_ua)
        .post(API_URL)
        .xhr()
        .header("Content-Type", "application/json")
        .body(body.to_string().into_bytes())
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let value: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(POPULAR_FIXTURE).unwrap());
    let entries = value
        .pointer("/data/ranking/totalRanking")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(parse_manga_item)
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    parse_search(body, "/data/serialPerDayOfWeek/panels")
}

fn parse_search(body: &str, connection_pointer: &str) -> Paged<CatalogItem> {
    let value: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).unwrap());
    let connection = value.pointer(connection_pointer).unwrap_or(&Value::Null);
    let entries = connection
        .get("edges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|edge| {
            edge.pointer("/node/storyInfo/magazine")
                .or_else(|| edge.get("node"))
                .and_then(parse_manga_item)
        })
        .collect();
    Paged {
        entries,
        has_next_page: connection
            .pointer("/pageInfo/hasNextPage")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn details_from_alias(alias: &str) -> CatalogItem {
    let body = graphql_or_fixture(
        "magazineDetail",
        HASH_MAGAZINE_DETAIL,
        json!({"magazineIdOrAlias": alias}),
        DETAILS_FIXTURE,
        false,
    );
    let value: Value = serde_json::from_str(&body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    let manga = value.pointer("/data/magazine").unwrap_or(&Value::Null);
    CatalogItem {
        key: alias.to_string(),
        title: string_at(manga, "/title").unwrap_or_else(|| alias.to_string()),
        cover: string_at(manga, "/squareWithLogoImageURL")
            .or_else(|| string_at(manga, "/rectangleWithLogoImageURL")),
        description: string_at(manga, "/description"),
        authors: string_at(manga, "/authorName")
            .map(|author| vec![author])
            .unwrap_or_default(),
        tags: manga
            .get("magazineTags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tag| string_at(tag, "/name"))
            .collect(),
        status: if manga.get("isFinished").and_then(Value::as_bool) == Some(true) {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(format!("{BASE_URL}/web/magazine/{alias}")),
        language: Some("ja".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, alias: &str) -> Vec<MangaChapter> {
    let value: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).unwrap());
    value
        .pointer("/data/magazine/storyInfos/edges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|edge| {
            let node = edge.get("node")?;
            let story_id = string_at(node, "/storyId")?;
            let title = string_at(node, "/title").unwrap_or_else(|| "Chapter".to_string());
            let subtitle = string_at(node, "/subtitle").unwrap_or_default();
            let name = if subtitle.is_empty() {
                title
            } else {
                format!("{title} {subtitle}")
            };
            let is_locked = node.get("isPurchased").and_then(Value::as_bool) == Some(false)
                && node
                    .pointer("/contentsAccessCondition/__typename")
                    .and_then(Value::as_str)
                    != Some("FreeStoryContentsAccessCondition");
            let key = format!("{alias}/{story_id}");
            Some(MangaChapter {
                key: key.clone(),
                title: Some(if is_locked { format!("Locked {name}") } else { name }),
                url: Some(format!("{BASE_URL}/web/reader/{key}/0")),
                is_locked,
                date_uploaded: node.get("contentsRelease").and_then(Value::as_i64),
                ..MangaChapter::default()
            })
        })
        .rev()
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let value: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).unwrap());
    let content = value.pointer("/data/magazine/storyContents").unwrap_or(&Value::Null);
    let Some(images) = content.get("pageImages") else {
        return Vec::new();
    };
    let count = images.get("pageCount").and_then(Value::as_u64).unwrap_or(0);
    let base = string_at(images, "/pageImageBaseURL").unwrap_or_default();
    let sign = string_at(images, "/pageImageSign").unwrap_or_default();
    let mut pages = (1..=count)
        .map(|page| {
            let image = if sign.is_empty() {
                format!("{base}{page}.jpg?w=4999")
            } else {
                format!("{base}{page}.jpg?{sign}&w=4999")
            };
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {page}")),
                ..MangaPage::default()
            }
        })
        .collect::<Vec<_>>();
    if let Some(afterword) = string_at(content, "/afterword/imageURL") {
        pages.push(MangaPage {
            content: PageContent::Url {
                url: format!("{afterword}?w=4999"),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some("Afterword".to_string()),
            ..MangaPage::default()
        });
    }
    pages
}

fn parse_manga_item(item: &Value) -> Option<CatalogItem> {
    let alias = string_at(item, "/alias")?;
    Some(CatalogItem {
        key: alias.clone(),
        title: string_at(item, "/title").unwrap_or_else(|| alias.clone()),
        cover: string_at(item, "/todaysJacketImageURL")
            .or_else(|| string_at(item, "/rectangleWithLogoImageURL")),
        url: Some(format!("{BASE_URL}/web/magazine/{alias}")),
        language: Some("ja".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn alias_from_input(input: &str) -> Option<String> {
    if input.is_empty() {
        return None;
    }
    if !input.starts_with(BASE_URL) {
        return Some(input.trim_matches('/').to_string()).filter(|alias| !alias.is_empty());
    }
    input
        .split("/web/magazine/")
        .nth(1)
        .or_else(|| input.split("/web/reader/").nth(1))
        .and_then(|value| value.split('/').next())
        .map(str::to_string)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

fn string_at(value: &Value, pointer: &str) -> Option<String> {
    value.pointer(pointer).and_then(|value| match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    })
}

export_manga_source!(SOURCE);

const HASH_HOME: &str = "b65659a4a5689bac97168591122219b69ee089d840b0415ace241d0caebee900";
const HASH_MAGAZINE_DETAIL: &str = "9a1460a42f8d04c70b23bb9ad763d0dbef2eb6f5d05dafca98ca2be8a2bfe867";
const HASH_STORY_INFO_LIST: &str = "acd460c52a231029d09e1ccca0aa06b99ae8163d5edff661cd64984ebb6dc4c3";
const HASH_MAGAZINE_STORY_FOR_READER: &str = "44e35d8af09515a315b06090723b72753828cf799466e3e1d722786844676617";
const HASH_MAGAZINES_BY_KEYWORD_SEARCH: &str = "55c7ca6cce30d8abdb0b32d00ad678ba37c03dd9b4851daf5ab5df5d41ce3ccc";
const HASH_FINISHED_MAGAZINES: &str = "ade49c46df5ef36f15485df70f656fb14f3261e90863fcd9ffbcc10baf30bc4c";
const HASH_SERIAL_MAGAZINES_BY_DAY_OF_WEEK: &str = "f1778757c51a4f8b59d91032096dd11b2071cb4191ca1904744672c814d16a97";

const POPULAR_FIXTURE: &str = r#"{"data":{"ranking":{"totalRanking":[{"alias":"sample","title":"Sample GANMA","todaysJacketImageURL":"https://ganma.jp/sample.jpg","rectangleWithLogoImageURL":null}]}}}"#;
const LATEST_FIXTURE: &str = r#"{"data":{"serialPerDayOfWeek":{"panels":{"edges":[{"node":{"storyInfo":{"magazine":{"alias":"sample","title":"Sample GANMA","todaysJacketImageURL":"https://ganma.jp/sample.jpg","rectangleWithLogoImageURL":null}}}}],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}"#;
const SEARCH_FIXTURE: &str = r#"{"data":{"searchComic":{"edges":[{"node":{"alias":"sample","title":"Sample GANMA","todaysJacketImageURL":"https://ganma.jp/sample.jpg","rectangleWithLogoImageURL":null}}],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}"#;
const FINISHED_FIXTURE: &str = r#"{"data":{"magazinesByCategory":{"magazines":{"edges":[{"node":{"alias":"sample","title":"Sample GANMA","todaysJacketImageURL":"https://ganma.jp/sample.jpg","rectangleWithLogoImageURL":null}}],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"magazine":{"alias":"sample","title":"Sample GANMA","authorName":"GANMA Author","description":"A sample title.","isFinished":false,"squareWithLogoImageURL":"https://ganma.jp/sample.jpg","rectangleWithLogoImageURL":null,"magazineTags":[{"name":"Action"}],"isWebOnlySensitive":false}}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"magazine":{"storyInfos":{"edges":[{"node":{"storyId":"story-1","title":"Episode 1","subtitle":"Start","contentsRelease":1767225600,"isPurchased":true,"contentsAccessCondition":{"__typename":"FreeStoryContentsAccessCondition","info":{"coins":0}}}}]}}}}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"magazine":{"storyContents":{"pageImages":{"pageCount":2,"pageImageBaseURL":"https://ganma.jp/pages/","pageImageSign":"token=sample"},"error":null,"afterword":{"imageURL":"https://ganma.jp/afterword.jpg"}}}}}"#;
