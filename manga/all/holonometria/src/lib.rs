use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const BASE_URL: &str = "https://holoearth.com";
const SOURCE: Holonometria = Holonometria;

struct Holonometria;

impl MangaSource for Holonometria {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let body = fetch_document_or_fixture(&list_url(source), LIST_FIXTURE);
        Ok(Paged { entries: parse_listing(&body, source), has_next_page: false })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_document_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged { entries: vec![parse_details(&body, Some(key), source)], has_next_page: false });
        }
        let body = fetch_document_or_fixture(&list_url(source), LIST_FIXTURE);
        let needle = query.to_ascii_lowercase();
        Ok(Paged {
            entries: parse_listing(&body, source)
                .into_iter()
                .filter(|item| needle.is_empty() || item.title.to_ascii_lowercase().contains(&needle))
                .collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/alt/holonometria/manga/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key), source))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/alt/holonometria/manga/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/alt/holonometria/manga/sample/1".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        let mut pages = parse_pages(&body);
        pages.reverse();
        Ok(pages)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        let source = source_for(&request);
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_document_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key), source)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
    path_prefix: &'static str,
}

fn source_for(request: &Value) -> SourceConfig {
    let id = request.get("sourceId").or_else(|| request.get("source_id")).and_then(Value::as_str).unwrap_or("holonometria-ja");
    SOURCES.iter().copied().find(|source| source.id == id).unwrap_or(SOURCES[0])
}

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/"))
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn list_url(source: SourceConfig) -> String {
    format!("{BASE_URL}/{}alt/holonometria/manga/", source.path_prefix)
}

fn parse_listing(body: &str, source: SourceConfig) -> Vec<CatalogItem> {
    body.split("manga__item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "manga__title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "HOLONOMETRIA".into())),
                cover: html::attr_after(chunk, "<img", "src").map(|value| url::join_url(BASE_URL, &value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some(source.lang.into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, key: Option<String>, source: SourceConfig) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/alt/holonometria/manga/sample".into());
    let info = html::text_between(body, "manga-detail__person", "</div>").unwrap_or_default();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "alt-nav__met-sub-link is-current", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "HOLONOMETRIA".into())),
        cover: html::attr_after(body, "manga-detail__thumb", "src").map(|value| url::join_url(BASE_URL, &value)),
        description: html::text_between(body, "manga-detail__caption", "</").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
        authors: person_value(&info, &["manga", "gambar", "漫画"]).into_iter().collect(),
        artists: person_value(&info, &["script", "naskah", "脚本"]).into_iter().collect(),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(source.lang.into()),
        content_rating: Some("safe".into()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("manga-detail__list-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "manga-detail__list-title", "</").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("manga-detail__swiper-wrapper")
        .nth(1)
        .unwrap_or(body)
        .split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, src)| MangaPage {
            content: PageContent::Url { url: url::join_url(BASE_URL, &src), context: None },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn person_value(info: &str, labels: &[&str]) -> Option<String> {
    info.split("<br")
        .find(|line| labels.iter().any(|label| line.to_ascii_lowercase().contains(&label.to_ascii_lowercase())))
        .map(|line| html::strip_tags(line).replace("&amp;", "&"))
        .and_then(|line| line.split([':', '：']).nth(1).map(str::trim).map(ToString::to_string))
        .filter(|value| !value.is_empty())
}

fn normalize_key(input: &str) -> String {
    let path = input.trim_start_matches(BASE_URL).split('#').next().unwrap_or(input).split('?').next().unwrap_or(input).trim();
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig { id: "holonometria-ja", lang: "ja", path_prefix: "" },
    SourceConfig { id: "holonometria-en", lang: "en", path_prefix: "en/" },
    SourceConfig { id: "holonometria-id", lang: "id", path_prefix: "id/" },
];

const LIST_FIXTURE: &str = r#"
<div class="manga__item"><a href="https://holoearth.com/alt/holonometria/manga/sample"><img src="https://holoearth.com/cover.jpg"></a><div class="manga__title">Sample</div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="alt-nav__met-sub-link is-current">Sample</div>
<div class="manga-detail__thumb"><img src="https://holoearth.com/cover.jpg"></div>
<div class="manga-detail__caption">Sample description</div>
<div class="manga-detail__person">Manga: Sample Author<br>Script: Sample Artist</div>
<div class="manga-detail__list"><div class="manga-detail__list-item"><a href="https://holoearth.com/alt/holonometria/manga/sample/1"><span class="manga-detail__list-title">Chapter 1</span></a></div></div>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="manga-detail__swiper-wrapper"><img src="https://holoearth.com/1.jpg"><img src="https://holoearth.com/2.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_holonometria() {
        assert_eq!(parse_listing(LIST_FIXTURE, SOURCES[0]).len(), 1);
        assert_eq!(parse_details(DETAILS_FIXTURE, Some("/sample".into()), SOURCES[0]).title, "Sample");
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
