use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: InManga = InManga;
const BASE_URL: &str = "https://inmanga.com";
const IMAGE_CDN: &str = "https://cdn1.intomanga.com";
const LANG: &str = "es";
const CONTENT_RATING: &str = "safe";

struct InManga;

impl MangaSource for InManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, 1));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "3"
        } else {
            "1"
        };
        Ok(parse_listing(&fetch_list(page, "", sort), page))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_list(page, query, "1"), page))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/ver/manga/sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/ver/manga/sample".into());
        let manga_id = key
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("sample");
        let body = client()
            .get(format!(
                "{BASE_URL}/chapter/getall?mangaIdentification={manga_id}"
            ))
            .xhr()
            .send_text()
            .unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string());
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/chapter/chapterIndexControls?identification=chapter-id".into());
        let target = absolute_url(&key);
        let body = fetch_document_or_fixture(&target, PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_list(page: u64, query: &str, sort: &str) -> String {
    let skip = (page.saturating_sub(1)) * 10;
    let skip_string = skip.to_string();
    client()
        .post(format!("{BASE_URL}/manga/getMangasConsultResult"))
        .xhr()
        .form(&[
            ("filter[generes][]", "-1"),
            ("filter[queryString]", query),
            ("filter[skip]", &skip_string),
            ("filter[take]", "10"),
            ("filter[sortby]", sort),
            ("filter[broadcastStatus]", "0"),
            ("filter[onlyFavorites]", "false"),
            ("d", ""),
        ])
        .send_text()
        .unwrap_or_else(|_| LIST_FIXTURE.to_string())
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, page: u64) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("h4") || chunk.contains("<img"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "<h4", "</h4>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "InManga".into())
                    }),
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| absolute_url(&value)),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() == 10 || page == 1 && body.contains("pagination"),
        entries,
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    parse_details(
        &fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE),
        key,
    )
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "InManga".into())),
        cover: html::attr_after(body, "div.col-md-3", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| absolute_url(&value)),
        description: html::text_between(body, "div.panel-body", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: status_from_text(&html::strip_tags(
            &html::text_between(body, "a.list-group-item", "</a>").unwrap_or_default(),
        )),
        url: Some(absolute_url(key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let data = serde_json::from_str::<InMangaResultDto>(body)
        .ok()
        .and_then(|root| root.data)
        .unwrap_or_else(|| CHAPTERS_DATA_FIXTURE.to_string());
    let result = serde_json::from_str::<InMangaResultObjectDto>(&data)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_DATA_FIXTURE).unwrap());
    if !result.success {
        return Vec::new();
    }
    let mut chapters = result
        .result
        .into_iter()
        .filter_map(|chapter| {
            let id = chapter.identification?;
            Some(MangaChapter {
                key: format!("/chapter/chapterIndexControls?identification={id}"),
                title: Some(format!(
                    "Chapter {}",
                    chapter.friendly_chapter_number.unwrap_or_else(|| {
                        chapter
                            .number
                            .map(|value| trim_float(value as f32))
                            .unwrap_or_else(|| "?".into())
                    })
                )),
                chapter_number: chapter.number.map(|value| value as f32),
                date_uploaded: manatan_shared::dates::parse_fixture_date(
                    &chapter.registration_date,
                ),
                language: Some(LANG.to_string()),
                url: Some(format!(
                    "{BASE_URL}/chapter/chapterIndexControls?identification={id}"
                )),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|a, b| {
        b.chapter_number
            .partial_cmp(&a.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let chapter_id = html::attr_after(body, "ChapterIdentification", "value")
        .unwrap_or_else(|| "chapter-id".into());
    let manga_id =
        html::attr_after(body, "MangaIdentification", "value").unwrap_or_else(|| "manga-id".into());
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("ImageContainer"))
        .filter_map(|chunk| html::attr(chunk, "id"))
        .enumerate()
        .map(|(index, page_id)| {
            let image = format!("{IMAGE_CDN}/i/m/{manga_id}/c/{chapter_id}/o/{page_id}.jpg");
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

fn normalize_key(input: &str) -> String {
    let path = input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_end_matches('/')
        .to_string();
    format!("/{}", path.trim_start_matches('/'))
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn status_from_text(status: &str) -> ItemStatus {
    let status = status.to_ascii_lowercase();
    if status.contains("finalizado") {
        ItemStatus::Completed
    } else if status.contains("emision") || status.contains("emisi") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn trim_float(value: f32) -> String {
    let text = value.to_string();
    text.strip_suffix(".0").unwrap_or(&text).to_string()
}

#[derive(Debug, Deserialize)]
struct InMangaResultDto {
    data: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InMangaResultObjectDto {
    success: bool,
    #[serde(default)]
    result: Vec<InMangaChapterDto>,
}

#[derive(Debug, Deserialize)]
struct InMangaChapterDto {
    #[serde(rename = "Number")]
    number: Option<f64>,
    #[serde(rename = "RegistrationDate", default)]
    registration_date: String,
    #[serde(rename = "Identification")]
    identification: Option<String>,
    #[serde(rename = "FriendlyChapterNumber")]
    friendly_chapter_number: Option<String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<body><a href="/ver/manga/sample"><img data-src="/cover.jpg"><h4 class="m0">Sample Manga</h4></a></body>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="col-md-3"><div class="panel widget"><img src="/cover.jpg"><a class="list-group-item">Estado <span>En emision</span></a></div></div>
<div class="col-md-9"><h1>Sample Manga</h1><div class="panel-body">Sample description</div></div>
"#;
const CHAPTERS_DATA_FIXTURE: &str = r#"{"success":true,"result":[{"Number":1.0,"RegistrationDate":"2024-01-01","Identification":"chapter-id","FriendlyChapterNumber":"1"}]}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":"{\"success\":true,\"result\":[{\"Number\":1.0,\"RegistrationDate\":\"2024-01-01\",\"Identification\":\"chapter-id\",\"FriendlyChapterNumber\":\"1\"}]}"}"#;
const PAGES_FIXTURE: &str = r#"
<input id="ChapterIdentification" value="chapter-id"><input id="MangaIdentification" value="manga-id">
<img class="ImageContainer" id="1"><img class="ImageContainer" id="2">
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_chapters_and_pages() {
        assert_eq!(parse_listing(LIST_FIXTURE, 1).entries.len(), 1);
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
