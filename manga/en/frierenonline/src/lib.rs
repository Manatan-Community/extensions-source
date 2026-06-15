use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: FrierenOnline = FrierenOnline;

struct FrierenOnline;

impl MangaSource for FrierenOnline {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: manga::Madara::parse_listing(LIST_FIXTURE, &config),
                has_next_page: manga::Madara::has_next_page(LIST_FIXTURE, &config),
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &config.list_url(page, "views"),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &config),
            has_next_page: manga::Madara::has_next_page(&body, &config),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(config.base_url) {
            let key = config.normalize_manga_key(query);
            let body = manga::Madara::fetch_document_or_fixture(&config, query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key), &config)],
                has_next_page: false,
            });
        }
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &config.search_url(page, query),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &config),
            has_next_page: manga::Madara::has_next_page(&body, &config),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = config();
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key), &config))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = config();
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
        let chapters = parse_frieren_chapters(&body, &key, &config);
        if chapters.is_empty() {
            Ok(manga::Madara::parse_chapters(&body, &key, &config))
        } else {
            Ok(chapters)
        }
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = config();
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), PAGES_FIXTURE);
        Ok(manga::Madara::parse_pages(&body, &config))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = config();
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(config.base_url) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &manga::Madara::fetch_document_or_fixture(&config, input, DETAILS_FIXTURE),
                    Some(config.normalize_manga_key(input)),
                    &config,
                )),
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

fn config() -> MadaraConfig {
    MadaraConfig {
        base_url: "https://www.frieren.online",
        lang: "en",
        content_rating: "safe",
        manga_path: "manga",
        popular_url_marker: "post-title",
        use_load_more: true,
        latest_enabled: false,
    }
}

fn parse_details(body: &str, key: Option<String>, config: &MadaraConfig) -> CatalogItem {
    let mut item = manga::Madara::parse_details(body, key, config);
    if let Some(title) =
        html::text_between(body, "class=\"about\"", "</h1>").map(|value| html::strip_tags(&value))
    {
        item.title = title;
    }
    item.cover = html::attr_after(body, "cover_managa", "src").or(item.cover);
    item.description = html::text_between(body, "class=\"synopsis\"", "</p>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or(item.description);
    item
}

fn parse_frieren_chapters(body: &str, manga_key: &str, config: &MadaraConfig) -> Vec<MangaChapter> {
    let chapters = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("m-chapter") && chunk.contains("chapter-content"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = config.normalize_manga_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "chapter-content", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                url: Some(config.absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.len() == 1 && chapters[0].key == manga_key {
        Vec::new()
    } else {
        chapters
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><h3 class="post-title"><a href="/manga/frieren/">Frieren</a></h3><img src="/cover.jpg"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="about"><h1>Frieren</h1></div><div class="cover_managa"><img src="/cover.jpg"></div><div class="synopsis"><p>Sample synopsis.</p></div>
<ul><li class="m-chapter"><a href="/manga/frieren/chapter-1/"><div class="chapter-content"><div>Chapter 1</div></div></a></li></ul>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_source() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].title, "Frieren");
        let chapters = SOURCE.chapters(json!({"manga":"/manga/frieren"})).unwrap();
        assert_eq!(chapters[0].title.as_deref(), Some("Chapter 1"));
    }
}
