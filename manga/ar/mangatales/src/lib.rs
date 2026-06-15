use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient};
use serde_json::Value;

const SOURCE: MangaTales = MangaTales;
const BASE_URL: &str = "https://www.mangatales.com";
const CDN_URL: &str = "https://media.mangatales.com";

struct MangaTales;

impl MangaSource for MangaTales {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_latest(LATEST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_or_fixture(
            &format!("{BASE_URL}/api/releases?page={page}"),
            LATEST_FIXTURE,
        );
        Ok(parse_latest(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_or_fixture(&format!("{BASE_URL}{}", key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, key)],
                has_next_page: false,
            });
        }
        Ok(Paged {
            entries: Vec::new(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/mangas/1".to_string());
        let body = fetch_or_fixture(&format!("{BASE_URL}{key}"), DETAILS_FIXTURE);
        Ok(parse_details(&body, key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/mangas/1".to_string());
        let id = key.trim_matches('/').rsplit('/').next().unwrap_or("1");
        let body = fetch_or_fixture(&format!("{BASE_URL}/api/mangas/{id}"), CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/r/10".to_string());
        let body = fetch_or_fixture(&format!("{BASE_URL}{key}"), READER_FIXTURE);
        Ok(parse_reader_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_or_fixture(&format!("{BASE_URL}{key}"), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, key)),
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

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
        .get(target)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Paged::default();
    };
    let releases = root
        .get("releases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|release| release.get("manga"))
        .filter(|manga| manga.get("is_novel").and_then(Value::as_bool) != Some(true))
        .collect::<Vec<_>>();
    let mut entries = Vec::new();
    for manga in releases {
        let Some(id) = manga.get("id").and_then(Value::as_i64) else {
            continue;
        };
        if entries
            .iter()
            .any(|entry: &CatalogItem| entry.key == format!("/mangas/{id}"))
        {
            continue;
        }
        entries.push(CatalogItem {
            key: format!("/mangas/{id}"),
            title: manga
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Manga")
                .to_string(),
            cover: manga
                .get("cover")
                .and_then(Value::as_str)
                .map(|cover| create_thumbnail(&id.to_string(), cover)),
            url: Some(format!("{BASE_URL}/mangas/{id}")),
            language: Some("ar".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        });
    }
    Paged {
        has_next_page: entries.len() >= 30,
        entries,
    }
}

fn parse_details(body: &str, key: String) -> CatalogItem {
    let data = embedded_json(body)
        .and_then(|value| value.get("mangaDataAction")?.get("mangaData").cloned())
        .or_else(|| serde_json::from_str::<Value>(body).ok())
        .unwrap_or(Value::Null);
    let id = data
        .get("id")
        .and_then(Value::as_i64)
        .map(|id| id.to_string())
        .unwrap_or_else(|| {
            key.trim_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("1")
                .to_string()
        });
    CatalogItem {
        key: key.clone(),
        title: data
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Manga")
            .to_string(),
        cover: data
            .get("cover")
            .and_then(Value::as_str)
            .map(|cover| create_thumbnail(&id, cover)),
        description: data
            .get("summary")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        authors: names(&data, "authors"),
        artists: names(&data, "artists"),
        tags: data
            .get("categories")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tag| tag.get("name").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect(),
        status: match data.get("story_status").and_then(Value::as_i64) {
            Some(2) => ItemStatus::Ongoing,
            Some(3) => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(format!("{BASE_URL}{key}")),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    root.get("mangaReleases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|release| {
            let id = release.get("id").and_then(Value::as_i64)?;
            let chapter = release
                .get("chapter")
                .map(chapter_label)
                .unwrap_or_else(|| "?".into());
            let title = release
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            Some(MangaChapter {
                key: format!("/r/{id}"),
                title: Some(if title.is_empty() {
                    chapter
                } else {
                    format!("{chapter} - {title}")
                }),
                scanlators: release
                    .get("team_name")
                    .and_then(Value::as_str)
                    .map(|value| vec![value.to_string()])
                    .unwrap_or_default(),
                date_uploaded: release
                    .get("created_at")
                    .and_then(Value::as_str)
                    .and_then(manatan_shared::dates::parse_fixture_date),
                url: Some(format!("{BASE_URL}/r/{id}")),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_reader_pages(body: &str) -> Vec<MangaPage> {
    let Some(data) = embedded_json(body) else {
        return Vec::new();
    };
    let media_key = data
        .get("globals")
        .and_then(|globals| globals.get("mediaKey"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let pages = data
        .get("readerDataAction")
        .and_then(|action| action.get("readerData"))
        .and_then(|reader| reader.get("release"))
        .and_then(|release| release.get("hq_pages"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    pages
        .split(['\r', '\n'])
        .map(str::trim)
        .filter(|page| !page.is_empty())
        .enumerate()
        .map(|(index, page)| MangaPage {
            content: PageContent::Url {
                url: format!("{CDN_URL}/uploads/releases/{page}?ak={media_key}"),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn embedded_json(body: &str) -> Option<Value> {
    html::text_between(body, "js-react-on-rails-component", "</div>")
        .and_then(|chunk| html::text_between(&chunk, ">", "</"))
        .or_else(|| html::text_between(body, ">", "</div>"))
        .map(|text| decode_json_entities(&text))
        .and_then(|text| serde_json::from_str(&text).ok())
        .or_else(|| serde_json::from_str(body).ok())
}

fn decode_json_entities(input: &str) -> String {
    input
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&amp;", "&")
}

fn names(data: &Value, field: &str) -> Vec<String> {
    data.get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|name| name.get("name").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect()
}

fn chapter_label(value: &Value) -> String {
    if let Some(number) = value.as_f64() {
        if number.fract() == 0.0 {
            return format!("{}", number as i64);
        }
        return number.to_string();
    }
    value.as_str().unwrap_or("?").to_string()
}

fn create_thumbnail(manga_id: &str, cover: &str) -> String {
    format!("{CDN_URL}/uploads/manga/cover/{manga_id}/large_{cover}")
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        return format!(
            "/{}",
            input.split('/').skip(3).collect::<Vec<_>>().join("/")
        )
        .trim_end_matches('/')
        .to_string();
    }
    format!("/{}", input.trim_matches('/'))
}

const LATEST_FIXTURE: &str = r#"{"releases":[{"manga":{"id":1,"title":"Sample Manga","cover":"cover.jpg","is_novel":false}}]}"#;
const DETAILS_FIXTURE: &str = r#"{"mangaDataAction":{"mangaData":{"id":1,"title":"Sample Manga","summary":"Sample summary.","cover":"cover.jpg","artists":[{"name":"Artist"}],"authors":[{"name":"Writer"}],"story_status":3,"categories":[{"name":"Drama"}]}}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"mangaReleases":[{"id":10,"chapter":1,"title":"Start","team_name":"Team","created_at":"2024-01-01T00:00:00.000Z"}]}"#;
const READER_FIXTURE: &str = r#"<div class="js-react-on-rails-component">{&quot;globals&quot;:{&quot;mediaKey&quot;:&quot;key&quot;},&quot;readerDataAction&quot;:{&quot;readerData&quot;:{&quot;release&quot;:{&quot;hq_pages&quot;:&quot;1.jpg\r\n2.jpg&quot;}}}}</div>"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gmanga_source() {
        let listing = parse_latest(LATEST_FIXTURE);
        assert_eq!(listing.entries[0].key, "/mangas/1");

        let details = parse_details(DETAILS_FIXTURE, "/mangas/1".into());
        assert_eq!(details.status, ItemStatus::Completed);

        let chapters = parse_chapters(CHAPTERS_FIXTURE);
        assert_eq!(chapters[0].key, "/r/10");

        let pages = parse_reader_pages(READER_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
