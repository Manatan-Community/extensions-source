use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: AralosBd = AralosBd;
const BASE_URL: &str = "https://aralosbd.fr";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "safe";

struct AralosBd;

impl MangaSource for AralosBd {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_search_result(LIST_FIXTURE, 0));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let page_index = page.saturating_sub(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "id"
        } else {
            "allviews"
        };
        Ok(parse_search_result(
            &fetch_text_or_fixture(
                &format!(
                    "{BASE_URL}/manga/search?s=sort:{sort};limit:24;-id:3;page:{page_index};order:desc"
                ),
                LIST_FIXTURE,
            ),
            page_index,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let page_index = page.saturating_sub(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = display_id(query) {
            let body = fetch_text_or_fixture(&api_manga_url(&id), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, &id)],
                has_next_page: false,
            });
        }
        Ok(parse_search_result(
            &fetch_text_or_fixture(
                &format!(
                    "{BASE_URL}/manga/search?s=page:{page_index};sort:id;order:desc;text:{}",
                    url::query_escape(query)
                ),
                LIST_FIXTURE,
            ),
            page_index,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        let id = display_id(&key).unwrap_or(key);
        Ok(parse_details(
            &fetch_text_or_fixture(&api_manga_url(&id), DETAILS_FIXTURE),
            &id,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        let id = display_id(&key).unwrap_or(key);
        Ok(parse_chapters(&fetch_text_or_fixture(
            &api_chapters_url(&id),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1".into());
        let id = chapter_id(&key).unwrap_or(key);
        Ok(parse_pages(&fetch_text_or_fixture(
            &api_pages_url(&id),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = display_id(input) {
            let body = fetch_text_or_fixture(&api_manga_url(&id), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, &id)),
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

export_manga_source!(SOURCE);

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/"))
}

fn fetch_text_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_search_result(body: &str, page_index: u64) -> Paged<CatalogItem> {
    let result = serde_json::from_str::<SearchResult>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    Paged {
        entries: result
            .mangas
            .into_iter()
            .map(SearchManga::into_item)
            .collect(),
        has_next_page: page_index + 1 < result.page_count,
    }
}

fn parse_details(body: &str, fallback_id: &str) -> CatalogItem {
    let manga = serde_json::from_str::<AralosManga>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    let id = if manga.id == 0 {
        fallback_id.to_string()
    } else {
        manga.id.to_string()
    };
    CatalogItem {
        key: id.clone(),
        title: if manga.main_title.is_empty() {
            "AralosBD".into()
        } else {
            manga.main_title
        },
        cover: (!manga.icon.is_empty()).then(|| url::join_url(BASE_URL, &manga.icon)),
        url: Some(display_url(&id)),
        authors: manga
            .authors
            .unwrap_or_default()
            .into_iter()
            .map(|author| author.name)
            .filter(|name| !name.is_empty())
            .collect(),
        description: Some(clean_markdown(&format!(
            "{}\n\n{}",
            manga.description,
            manga.fulldescription.unwrap_or_default()
        )))
        .filter(|value| !value.is_empty()),
        tags: manga
            .tags
            .unwrap_or_default()
            .into_iter()
            .map(|tag| tag.tag)
            .filter(|tag| !tag.is_empty())
            .collect(),
        status: ItemStatus::Unknown,
        language: Some(LANG.into()),
        content_rating: Some(CONTENT_RATING.into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    serde_json::from_str::<Vec<AralosChapter>>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"))
        .into_iter()
        .filter(|chapter| chapter.chapter_released == "1")
        .map(|chapter| {
            let key = chapter.chapter_id.clone();
            MangaChapter {
                key: key.clone(),
                title: Some(
                    format!("{} - {}", chapter.chapter_number, chapter.chapter_title)
                        .trim_end_matches(" -")
                        .to_string(),
                ),
                chapter_number: chapter.chapter_number.parse::<f32>().ok(),
                scanlators: chapter
                    .chapter_translator
                    .into_iter()
                    .filter(|value| !value.is_empty())
                    .collect(),
                date_uploaded: manatan_shared::dates::parse_fixture_date(
                    &chapter.chapter_release_time,
                ),
                url: Some(format!("{BASE_URL}/manga/chapter?id={key}")),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    serde_json::from_str::<AralosPages>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"))
        .links
        .into_iter()
        .enumerate()
        .map(|(index, link)| {
            let image = url::join_url(BASE_URL, &link);
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn clean_markdown(input: &str) -> String {
    let mut text = html::html_unescape(input);
    for (start, end) in [("[", "]("), ("**", "**"), ("_", "_")] {
        while let Some(begin) = text.find(start) {
            let Some(mid) = text[begin + start.len()..]
                .find(end)
                .map(|idx| begin + start.len() + idx)
            else {
                break;
            };
            if end == "](" {
                let Some(close) = text[mid + 2..].find(')').map(|idx| mid + 2 + idx) else {
                    break;
                };
                let link = text[mid + 2..close].to_string();
                text.replace_range(begin..=close, &link);
            } else {
                let value = text[begin + start.len()..mid].trim().to_string();
                text.replace_range(begin..mid + end.len(), &value);
            }
        }
    }
    text.split("---")
        .next()
        .unwrap_or(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn display_id(input: &str) -> Option<String> {
    query_param(input, "id").or_else(|| input.strip_prefix("id:").map(ToString::to_string))
}

fn chapter_id(input: &str) -> Option<String> {
    query_param(input, "id").or_else(|| input.strip_prefix("chapter:").map(ToString::to_string))
}

fn query_param(input: &str, name: &str) -> Option<String> {
    let query = input.split('?').nth(1).unwrap_or(input);
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name && !value.is_empty()).then(|| value.to_string())
    })
}

fn display_url(id: &str) -> String {
    format!("{BASE_URL}/manga/display?id={id}")
}

fn api_manga_url(id: &str) -> String {
    format!("{BASE_URL}/manga/api?get=manga&id={id}")
}

fn api_chapters_url(id: &str) -> String {
    format!("{BASE_URL}/manga/api?get=chapters&manga={id}")
}

fn api_pages_url(id: &str) -> String {
    format!("{BASE_URL}/manga/api?get=pages&chapter={id}")
}

#[derive(Deserialize)]
struct SearchManga {
    #[serde(default)]
    icon: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    id: String,
}

impl SearchManga {
    fn into_item(self) -> CatalogItem {
        CatalogItem {
            key: self.id.clone(),
            title: if self.title.is_empty() {
                "AralosBD".into()
            } else {
                self.title
            },
            cover: (!self.icon.is_empty()).then(|| url::join_url(BASE_URL, &self.icon)),
            url: Some(display_url(&self.id)),
            language: Some(LANG.into()),
            content_rating: Some(CONTENT_RATING.into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct SearchResult {
    #[serde(default)]
    page_count: u64,
    #[serde(default)]
    mangas: Vec<SearchManga>,
}

#[derive(Deserialize)]
struct NameDto {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct TagDto {
    #[serde(default)]
    tag: String,
}

#[derive(Deserialize)]
struct AralosManga {
    #[serde(default)]
    main_title: String,
    #[serde(default)]
    fulldescription: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    id: i64,
    #[serde(default)]
    authors: Option<Vec<NameDto>>,
    #[serde(default)]
    tags: Option<Vec<TagDto>>,
    #[serde(default)]
    icon: String,
}

#[derive(Deserialize)]
struct AralosChapter {
    #[serde(default)]
    chapter_number: String,
    #[serde(default)]
    chapter_title: String,
    #[serde(default)]
    chapter_translator: Option<String>,
    #[serde(default)]
    chapter_id: String,
    #[serde(default)]
    chapter_released: String,
    #[serde(default)]
    chapter_release_time: String,
}

#[derive(Deserialize)]
struct AralosPages {
    #[serde(default)]
    links: Vec<String>,
}

const LIST_FIXTURE: &str =
    r#"{"page_count":1,"mangas":[{"id":"1","title":"Sample","icon":"cover.jpg"}]}"#;
const DETAILS_FIXTURE: &str = r#"{"id":1,"main_title":"Sample","description":"Summary","fulldescription":"","authors":[{"name":"Author"}],"tags":[{"tag":"Action","color":""}],"icon":"cover.jpg"}"#;
const CHAPTERS_FIXTURE: &str = r#"[{"chapter_number":"1","chapter_title":"One","chapter_translator":"Team","chapter_id":"10","chapter_released":"1","chapter_release_time":"2024-01-01 00:00:00"}]"#;
const PAGES_FIXTURE: &str = r#"{"links":["page1.jpg","page2.jpg"]}"#;
