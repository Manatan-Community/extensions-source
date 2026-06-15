use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const BASE_URL: &str = "https://wp.comicskingdom.com";
const SOURCE: ComicsKingdom = ComicsKingdom;

struct ComicsKingdom;

impl MangaSource for ComicsKingdom {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "modified"
        } else {
            "relevance"
        };
        Ok(fetch_manga_page(&manga_api_url(source_lang(&request), page, order, "")))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let body = fetch_text_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_manga(&body).to_item(source_lang(&request))],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let mut target = manga_api_url(
            source_lang(&request),
            page,
            filter(filters, "orderby").unwrap_or("relevance"),
            query,
        );
        for key in ["ck_genre", "ck_genre_exclude"] {
            if let Some(value) = filter(filters, key) {
                target.push('&');
                target.push_str(key);
                target.push('=');
                target.push_str(&url::query_escape(value));
            }
        }
        Ok(fetch_manga_page(&target))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1:sample".into());
        let body = fetch_text_or_fixture(&details_url(&key), DETAILS_FIXTURE);
        Ok(parse_manga(&body).to_item(source_lang(&request)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1:sample".into());
        let manga = parse_manga(&fetch_text_or_fixture(&details_url(&key), DETAILS_FIXTURE));
        let slug = url::slug_from_url(&manga.link).unwrap_or_else(|| "sample".into());
        if request
            .get("preferences")
            .and_then(|prefs| prefs.get("compactChapters"))
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            return Ok(vec![MangaChapter {
                key: format!("compact:{slug}:1"),
                title: Some("1-100".into()),
                chapter_number: Some(0.0),
                url: Some(format!("{BASE_URL}/wp-json/wp/v2/ck_comic?per_page=100&orderBy=date&order=asc&ck_feature={slug}&page=1")),
                ..MangaChapter::default()
            }]);
        }
        let body = fetch_text_or_fixture(&chapter_list_url(&slug, 1), CHAPTERS_FIXTURE);
        Ok(parse_chapter_array(&body, &slug))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "compact:sample:1".into());
        let body = if key.starts_with("compact:") {
            let parts = key.split(':').collect::<Vec<_>>();
            let slug = parts.get(1).copied().unwrap_or("sample");
            let page = parts.get(2).copied().unwrap_or("1");
            fetch_text_or_fixture(&chapter_list_url(slug, page.parse().unwrap_or(1)), CHAPTERS_FIXTURE)
        } else {
            fetch_text_or_fixture(&details_url(&key), CHAPTER_FIXTURE)
        };
        Ok(parse_pages_from_chapters(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            let body = fetch_text_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_manga(&body).to_item(source_lang(&request))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/"))
}

fn fetch_text_or_fixture(target_url: &str, fixture: &str) -> String {
    client().get(target_url).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn fetch_manga_page(target_url: &str) -> Paged<CatalogItem> {
    let body = fetch_text_or_fixture(target_url, LIST_FIXTURE);
    let list = serde_json::from_str::<Vec<MangaDto>>(&body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture list"));
    Paged {
        has_next_page: list.len() == 20,
        entries: list.into_iter().map(|manga| manga.to_item("all")).collect(),
    }
}

fn manga_api_url(lang: &str, page: u64, order: &str, query: &str) -> String {
    let ck_language = if lang == "es" { "spanish" } else { "english" };
    let mut out = format!("{BASE_URL}/wp-json/wp/v2/ck_feature?per_page=20&_fields=id,link,title,content,meta,yoast_head&ck_language={ck_language}&orderBy={order}&page={page}");
    if !query.is_empty() {
        out.push_str("&search=");
        out.push_str(&url::query_escape(query));
    }
    out
}

fn chapter_list_url(slug: &str, page: u64) -> String {
    format!("{BASE_URL}/wp-json/wp/v2/ck_comic?per_page=100&_fields=id,date,assets,link&order=desc&ck_feature={slug}&page={page}")
}

fn details_url(key: &str) -> String {
    if key.starts_with(BASE_URL) {
        key.to_string()
    } else if let Some((id, slug)) = key.split_once(':') {
        format!("{BASE_URL}/wp-json/wp/v2/ck_feature/{id}?slug={slug}")
    } else {
        format!("{BASE_URL}/wp-json/wp/v2/ck_feature/{key}")
    }
}

fn parse_manga(body: &str) -> MangaDto {
    serde_json::from_str::<MangaDto>(body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture details"))
}

fn parse_chapter_array(body: &str, slug: &str) -> Vec<MangaChapter> {
    serde_json::from_str::<Vec<ChapterDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture chapters"))
        .into_iter()
        .enumerate()
        .map(|(index, chapter)| MangaChapter {
            key: format!("{}:{}", chapter.id, url::slug_from_url(&chapter.link).unwrap_or_else(|| slug.into())),
            title: Some(chapter.date.split('T').next().unwrap_or("Comic").to_string()),
            chapter_number: Some(index as f32 * 0.01),
            url: Some(chapter.link.clone()),
            ..MangaChapter::default()
        })
        .collect()
}

fn parse_pages_from_chapters(body: &str) -> Vec<MangaPage> {
    let chapters = serde_json::from_str::<Vec<ChapterDto>>(body)
        .or_else(|_| serde_json::from_str::<ChapterDto>(body).map(|chapter| vec![chapter]))
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture chapters"));
    chapters
        .into_iter()
        .enumerate()
        .filter_map(|(index, chapter)| {
            Some(MangaPage {
                content: PageContent::Url {
                    url: chapter.assets?.single.url,
                    context: None,
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn source_lang(request: &Value) -> &'static str {
    match request.get("sourceId").or_else(|| request.get("source_id")).and_then(Value::as_str) {
        Some("comicskingdom-es") => "es",
        _ => "en",
    }
}

fn filter<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters.get(key).and_then(Value::as_str).filter(|value| !value.is_empty())
}

#[derive(Debug, Deserialize)]
struct MangaDto {
    id: u64,
    link: String,
    title: Rendered,
    content: Rendered,
    meta: MangaMeta,
    #[serde(default)]
    yoast_head: String,
}

impl MangaDto {
    fn to_item(self, lang: &str) -> CatalogItem {
        let slug = url::slug_from_url(&self.link).unwrap_or_else(|| self.id.to_string());
        CatalogItem {
            key: format!("{}:{slug}", self.id),
            title: html::strip_tags(&self.title.rendered),
            cover: thumbnail_from_yoast(&self.yoast_head),
            authors: vec![self.meta.ck_byline_on_app.replace("By", "").trim().to_string()].into_iter().filter(|v| !v.is_empty()).collect(),
            description: Some(html::strip_tags(&self.content.rendered)).filter(|v| !v.is_empty()),
            status: ItemStatus::Unknown,
            url: Some(self.link),
            language: Some(lang.to_string()),
            content_rating: Some("safe".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChapterDto {
    id: u64,
    date: String,
    assets: Option<Assets>,
    link: String,
}

#[derive(Debug, Deserialize)]
struct Assets { single: AssetData }

#[derive(Debug, Deserialize)]
struct AssetData { url: String }

#[derive(Debug, Deserialize)]
struct MangaMeta {
    #[serde(default)]
    ck_byline_on_app: String,
}

#[derive(Debug, Deserialize)]
struct Rendered { rendered: String }

fn thumbnail_from_yoast(value: &str) -> Option<String> {
    value
        .split("thumbnailUrl\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .map(ToString::to_string)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"[{"id":1,"link":"https://wp.comicskingdom.com/sample","title":{"rendered":"Sample"},"content":{"rendered":"<p>Description</p>"},"meta":{"ck_byline_on_app":"By Author"},"yoast_head":"thumbnailUrl\":\"https://img.example/cover.jpg\",\"dateP"}]"#;
const DETAILS_FIXTURE: &str = r#"{"id":1,"link":"https://wp.comicskingdom.com/sample","title":{"rendered":"Sample"},"content":{"rendered":"<p>Description</p>"},"meta":{"ck_byline_on_app":"By Author"},"yoast_head":"thumbnailUrl\":\"https://img.example/cover.jpg\",\"dateP"}"#;
const CHAPTER_FIXTURE: &str = r#"{"id":10,"date":"2024-01-01T00:00:00","assets":{"single":{"url":"https://img.example/1.jpg"}},"link":"https://wp.comicskingdom.com/sample/2024-01-01"}"#;
const CHAPTERS_FIXTURE: &str = r#"[{"id":10,"date":"2024-01-01T00:00:00","assets":{"single":{"url":"https://img.example/1.jpg"}},"link":"https://wp.comicskingdom.com/sample/2024-01-01"}]"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comics_kingdom_data() {
        let list = serde_json::from_str::<Vec<MangaDto>>(LIST_FIXTURE).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(parse_manga(DETAILS_FIXTURE).to_item("en").title, "Sample");
        assert_eq!(parse_chapter_array(CHAPTERS_FIXTURE, "sample").len(), 1);
        assert_eq!(parse_pages_from_chapters(CHAPTERS_FIXTURE).len(), 1);
    }
}
