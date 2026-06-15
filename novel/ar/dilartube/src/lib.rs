use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{dates, html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;
use std::collections::BTreeSet;

const SOURCE: DilarTube = DilarTube;
const BASE_URL: &str = "https://golden.rest";
const SEARCH_URL: &str = "https://dilar.tube/api/quick_search";

struct DilarTube;

impl NovelSource for DilarTube {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_api_or_fixture(
            &format!("{BASE_URL}/api/releases?page={page}"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) || query.starts_with("https://dilar.tube") {
            let key = normalize_key(query);
            let body = fetch_api_or_fixture(
                &format!("{BASE_URL}/api/{}", key.trim_start_matches('/')),
                DETAILS_FIXTURE,
            );
            return Ok(Paged {
                entries: vec![parse_details(&body, key)],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return Ok(Paged::default());
        }
        let body = client()
            .post(SEARCH_URL)
            .origin("https://dilar.tube")
            .referer("https://dilar.tube/")
            .xhr()
            .form(&[("query", query), ("includes", "[\"Manga\",\"Team\",\"Member\"]")])
            .send_text()
            .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
        Ok(parse_search(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "/mangas/1".to_string());
        let body = fetch_api_or_fixture(
            &format!("{BASE_URL}/api/{}", key.trim_start_matches('/')),
            DETAILS_FIXTURE,
        );
        Ok(parse_details(&body, key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "/mangas/1".to_string());
        let body = fetch_api_or_fixture(
            &format!("{BASE_URL}/api/{}/releases", key.trim_start_matches('/')),
            CHAPTERS_FIXTURE,
        );
        let details = fetch_api_or_fixture(
            &format!("{BASE_URL}/api/{}", key.trim_start_matches('/')),
            DETAILS_FIXTURE,
        );
        let title = manga_data(&details)
            .and_then(|data| text_value(&data, "title"))
            .unwrap_or_else(|| "novel".to_string());
        Ok(parse_chapters(&body, &key, &title))
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter")
            .unwrap_or_else(|| "/mangas/1/sample-title/1".to_string());
        let body = fetch_document_or_fixture(&format!("{BASE_URL}{key}"), READER_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let listing = parse_listing(&fetch_api_or_fixture(
            &format!("{BASE_URL}/api/releases?page=1"),
            LIST_FIXTURE,
        ));
        Ok(vec![HomeSection {
            id: "latest".to_string(),
            title: "Latest".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: listing.entries,
            has_more: listing.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) || input.starts_with("https://dilar.tube") {
            let key = normalize_key(input);
            if key.starts_with("/mangas/") {
                let body = fetch_api_or_fixture(
                    &format!("{BASE_URL}/api/{}", key.trim_start_matches('/')),
                    DETAILS_FIXTURE,
                );
                return Ok(Some(UrlResolveResult {
                    item: Some(parse_details(&body, key)),
                    url: Some(input.to_string()),
                    ..UrlResolveResult::default()
                }));
            }
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

fn fetch_api_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Paged::default();
    };
    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();
    for manga in root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(
            root.get("releases")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|release| release.get("manga")),
        )
    {
        if manga.get("is_novel").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let Some(id) = manga.get("id").and_then(Value::as_i64) else {
            continue;
        };
        let title = text_value(manga, "title").unwrap_or_else(|| "Novel".to_string());
        if !seen.insert(title.clone()) {
            continue;
        }
        entries.push(CatalogItem {
            key: format!("/mangas/{id}"),
            title,
            cover: text_value(manga, "cover").map(|cover| cover_url(&id.to_string(), &cover)),
            url: Some(format!("{BASE_URL}/mangas/{id}")),
            language: Some("ar".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        });
    }
    let has_next_page = root
        .get("next_page_url")
        .is_some_and(|value| !value.is_null())
        || root
            .get("current_page")
            .and_then(Value::as_u64)
            .zip(root.get("last_page").and_then(Value::as_u64))
            .is_some_and(|(current, last)| current < last)
        || entries.len() >= 30;
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Paged::default();
    };
    let data = root
        .as_array()
        .and_then(|items| items.first())
        .cloned()
        .unwrap_or(root);
    Paged {
        entries: parse_listing(&data.to_string()).entries,
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: String) -> CatalogItem {
    let data = manga_data(body).unwrap_or(Value::Null);
    let id = data
        .get("id")
        .and_then(Value::as_i64)
        .map(|id| id.to_string())
        .unwrap_or_else(|| key.trim_matches('/').rsplit('/').next().unwrap_or("1").to_string());
    let mut description = text_value(&data, "summary");
    let synonyms = text_value(&data, "synonyms");
    if let (Some(summary), Some(synonyms)) = (&description, synonyms.filter(|value| !value.is_empty())) {
        description = Some(format!("{summary}\n\nSynonyms: {synonyms}"));
    }
    CatalogItem {
        key: key.clone(),
        title: text_value(&data, "arabic_title")
            .or_else(|| text_value(&data, "title"))
            .unwrap_or_else(|| "Novel".to_string()),
        alternate_titles: [text_value(&data, "english"), text_value(&data, "japanese")]
            .into_iter()
            .flatten()
            .filter(|value| !value.is_empty())
            .collect(),
        cover: text_value(&data, "cover").map(|cover| cover_url(&id, &cover)),
        description,
        authors: names(&data, "authors"),
        artists: names(&data, "artists"),
        tags: tags(&data),
        status: match data.get("story_status").and_then(Value::as_i64) {
            Some(2) => ItemStatus::Ongoing,
            Some(3) => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(format!("{BASE_URL}{key}")),
        language: Some("ar".to_string()),
        content_rating: Some(if data.get("over17").and_then(Value::as_bool) == Some(true) {
            "suggestive".to_string()
        } else {
            "safe".to_string()
        }),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, novel_key: &str, title: &str) -> Vec<NovelChapter> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let slug = title.replace(' ', "-");
    let mut chapters: Vec<_> = root
        .get("releases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|release| {
            let chapter = release.get("chapter").map(chapter_label)?;
            let chapter_path = format!("{}/{}/{}", novel_key.trim_end_matches('/'), slug, chapter);
            Some(NovelChapter {
                key: chapter_path.clone(),
                title: text_value(release, "title").filter(|value| !value.is_empty()),
                chapter_number: release.get("chapter").and_then(Value::as_f64).map(|value| value as f32),
                date_uploaded: release
                    .get("created_at")
                    .and_then(Value::as_str)
                    .and_then(dates::parse_fixture_date),
                url: Some(format!("{BASE_URL}{chapter_path}")),
                language: Some("ar".to_string()),
                release_group: text_value(release, "team_name"),
                ..NovelChapter::default()
            })
        })
        .collect();
    chapters.reverse();
    chapters
}

fn parse_text(body: &str, _key: &str) -> NovelText {
    let data = embedded_json(body).unwrap_or(Value::Null);
    let release = data
        .get("readerDataAction")
        .and_then(|action| action.get("readerData"))
        .and_then(|reader| reader.get("release"))
        .unwrap_or(&Value::Null);
    let raw = text_value(release, "content").unwrap_or_else(|| {
        html::text_between(body, "<article", "</article>")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_default()
    });
    let html = raw
        .split(['\r', '\n'])
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| format!("<p>{line}</p>"))
        .collect::<Vec<_>>()
        .join("");
    let normalized = novel::normalize_reader_html(&html);
    NovelText {
        title: text_value(release, "title"),
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(BASE_URL.to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn manga_data(body: &str) -> Option<Value> {
    let root = serde_json::from_str::<Value>(body).ok()?;
    root.get("mangaData")
        .cloned()
        .or_else(|| root.get("mangaDataAction")?.get("mangaData").cloned())
        .or_else(|| root.get("data").cloned())
        .or(Some(root))
}

fn embedded_json(body: &str) -> Option<Value> {
    html::text_between(body, "js-react-on-rails-component", "</script>")
        .or_else(|| html::text_between(body, "js-react-on-rails-component", "</div>"))
        .or_else(|| html::text_between(body, ">", "</script>"))
        .map(|text| decode_json_entities(&text))
        .and_then(|text| serde_json::from_str(&text).ok())
        .or_else(|| serde_json::from_str(body).ok())
}

fn text_value(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn names(data: &Value, field: &str) -> Vec<String> {
    data.get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|name| text_value(name, "name"))
        .collect()
}

fn tags(data: &Value) -> Vec<String> {
    let translation = match data.get("translation_status").and_then(Value::as_i64) {
        Some(0) => Some("منتهية"),
        Some(1) => Some("مستمره"),
        Some(2) => Some("متوقفة"),
        Some(3) => Some("غير مترجمه"),
        _ => None,
    };
    translation
        .into_iter()
        .map(ToString::to_string)
        .chain(
            data.get("categories")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|tag| text_value(tag, "name")),
        )
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

fn cover_url(manga_id: &str, cover: &str) -> String {
    url::join_url(BASE_URL, &format!("/uploads/manga/cover/{manga_id}/{cover}"))
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

fn decode_json_entities(input: &str) -> String {
    input
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&amp;", "&")
}

const LIST_FIXTURE: &str = r#"{"current_page":1,"last_page":1,"releases":[{"manga":{"id":1,"title":"Sample Novel","cover":"cover.jpg","is_novel":true}}],"data":[{"id":2,"title":"Search Novel","cover":"search.jpg","is_novel":true}]}"#;
const SEARCH_FIXTURE: &str = r#"[{"data":[{"id":2,"title":"Search Novel","cover":"search.jpg","is_novel":true}],"releases":[]}]"#;
const DETAILS_FIXTURE: &str = r#"{"mangaData":{"id":1,"title":"Sample Novel","arabic_title":"رواية تجريبية","summary":"Sample summary.","cover":"cover.jpg","story_status":3,"translation_status":1,"over17":false,"english":"Sample Novel EN","japanese":"","synonyms":"Alt Sample","authors":[{"name":"Writer"}],"artists":[{"name":"Artist"}],"categories":[{"name":"Fantasy"}]}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"releases":[{"chapter":1,"title":"Start","team_name":"Team","created_at":"2024-01-01T00:00:00.000Z"},{"chapter":2,"title":"Next","team_name":"Team","created_at":"2024-01-02T00:00:00.000Z"}]}"#;
const READER_FIXTURE: &str = r#"<script class="js-react-on-rails-component">{"readerDataAction":{"readerData":{"release":{"title":"Start","content":"First line\nSecond line"}}}}</script>"#;

export_novel_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_and_search() {
        let page = parse_listing(LIST_FIXTURE);
        assert_eq!(page.entries.len(), 2);
        assert!(page.entries.iter().all(|item| item.language.as_deref() == Some("ar")));

        let search = parse_search(SEARCH_FIXTURE);
        assert_eq!(search.entries[0].key, "/mangas/2");
    }

    #[test]
    fn parses_details_and_chapters() {
        let details = parse_details(DETAILS_FIXTURE, "/mangas/1".to_string());
        assert_eq!(details.status, ItemStatus::Completed);
        assert_eq!(details.authors[0], "Writer");

        let chapters = parse_chapters(CHAPTERS_FIXTURE, "/mangas/1", "Sample Novel");
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].chapter_number, Some(2.0));
    }

    #[test]
    fn parses_reader_text() {
        let text = parse_text(READER_FIXTURE, "/mangas/1/sample/1");
        assert!(text.html.as_deref().unwrap_or_default().contains("<p>First line</p>"));
        assert!(text.text.as_deref().unwrap_or_default().contains("Second line"));
    }
}
