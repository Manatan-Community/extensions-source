use manatan_extension::{
    abi::ExtensionResult, export_manga_source, source::MangaSource, CatalogItem, HomeSection,
    HomeSectionStyle, MangaChapter, MangaPage, Paged, UrlResolveResult,
};
use manatan_shared::{manga, manga::MadaraConfig, sdk::SearchRequest, url};
use regex::Regex;
use serde_json::Value;

const SOURCE: Hiperdex = Hiperdex;
const DEFAULT_BASE_URL: &str = "https://hiperdex.com";

struct Hiperdex;

impl MangaSource for Hiperdex {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config(&request);
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(clean_page(page_from_body(LIST_FIXTURE, &config), &request));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request
            .get("listingId")
            .or_else(|| request.get("listing"))
            .and_then(Value::as_str)
            == Some("latest")
        {
            "latest"
        } else {
            "views"
        };
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.list_url(page, order), LIST_FIXTURE);
        Ok(clean_page(page_from_body(&body, &config), &request))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(config.base_url) {
            let key = config.normalize_manga_key(query);
            let body = manga::Madara::fetch_document_or_fixture(&config, query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![clean_item(
                    manga::Madara::parse_details(&body, Some(key), &config),
                    &request,
                )],
                has_next_page: false,
            });
        }
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &config.search_url(page, query),
            LIST_FIXTURE,
        );
        Ok(clean_page(page_from_body(&body, &config), &request))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = config(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
        Ok(clean_item(
            manga::Madara::parse_details(&body, Some(key), &config),
            &request,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = config(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_chapters(&body, &key, &config))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = config(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        let body =
            manga::Madara::fetch_document_or_fixture(&config, &config.absolute_url(&key), PAGES_FIXTURE);
        Ok(manga::Madara::parse_pages(&body, &config))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let config = config(&request);
        let popular = clean_page(
            page_from_body(
                &manga::Madara::fetch_document_or_fixture(
                    &config,
                    &config.list_url(1, "views"),
                    LIST_FIXTURE,
                ),
                &config,
            ),
            &request,
        );
        let latest = clean_page(
            page_from_body(
                &manga::Madara::fetch_document_or_fixture(
                    &config,
                    &config.list_url(1, "latest"),
                    LIST_FIXTURE,
                ),
                &config,
            ),
            &request,
        );
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = config(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(config.base_url) || input.starts_with(DEFAULT_BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(clean_item(
                    manga::Madara::parse_details(
                        &manga::Madara::fetch_document_or_fixture(&config, input, DETAILS_FIXTURE),
                        Some(config.normalize_manga_key(input)),
                        &config,
                    ),
                    &request,
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

fn page_from_body(body: &str, config: &MadaraConfig) -> Paged<CatalogItem> {
    Paged {
        entries: manga::Madara::parse_listing(body, config),
        has_next_page: manga::Madara::has_next_page(body, config),
    }
}

fn clean_page(mut page: Paged<CatalogItem>, request: &Value) -> Paged<CatalogItem> {
    page.entries = page
        .entries
        .into_iter()
        .map(|item| clean_item(item, request))
        .collect();
    page
}

fn clean_item(mut item: CatalogItem, request: &Value) -> CatalogItem {
    let original = item.title.clone();
    let cleaned = clean_title(&original, request);
    if cleaned != original.trim() {
        if let Some(description) = item.description.filter(|value| !value.is_empty()) {
            item.description = Some(format!("{original}\n\n{description}"));
        } else {
            item.description = Some(original);
        }
        item.title = cleaned;
    }
    item
}

fn clean_title(title: &str, request: &Value) -> String {
    let mut out = title.to_string();
    if let Some(pattern) = pref_str(request, "removeTitleCustom").filter(|value| !value.is_empty())
    {
        if let Ok(regex) = Regex::new(&pattern) {
            out = regex.replace_all(&out, "").to_string();
        }
    }
    if pref_bool(request, "removeTitleVersion") {
        out = strip_version_tags(&out);
    }
    out.trim().to_string()
}

fn strip_version_tags(value: &str) -> String {
    let mut out = value.trim();
    loop {
        let trimmed = out.trim();
        if let Some(rest) = strip_wrapped_prefix(trimmed) {
            out = rest;
            continue;
        }
        if let Some(rest) = strip_wrapped_suffix(trimmed) {
            out = rest;
            continue;
        }
        if let Some(rest) = trimmed.strip_suffix("/ Official") {
            out = rest;
            continue;
        }
        return trimmed.to_string();
    }
}

fn strip_wrapped_prefix(value: &str) -> Option<&str> {
    let close = match value.chars().next()? {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        _ => return None,
    };
    let end = value.find(close)?;
    Some(value[end + close.len_utf8()..].trim_start())
}

fn strip_wrapped_suffix(value: &str) -> Option<&str> {
    let open = match value.chars().last()? {
        ')' => '(',
        ']' => '[',
        '}' => '{',
        _ => return None,
    };
    let start = value.rfind(open)?;
    Some(value[..start].trim_end())
}

fn config(request: &Value) -> MadaraConfig {
    let base_url = pref_str(request, "overrideBaseUrl")
        .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    MadaraConfig {
        base_url: Box::leak(base_url.trim_end_matches('/').to_string().into_boxed_str()),
        lang: "en",
        content_rating: "adult",
        manga_path: "manga",
        popular_url_marker: "post-title",
        use_load_more: false,
        latest_enabled: true,
    }
}

fn pref_str(request: &Value, key: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .map(ToString::to_string)
}

fn pref_bool(request: &Value, key: &str) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .or_else(|| request.get(key))
        .and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_str().map(|text| text == "true"))
        })
        .unwrap_or(false)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><h3 class="post-title"><a href="/manga/sample/">Sample Manga (Official)</a></h3><img src="/cover.jpg"></div>
<div class="nav-previous"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample Manga (Official)</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
<div class="description-summary"><p>Sample description.</p></div>
<ul class="main version-chap"><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">2024-01-01</span></li></ul>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cleans_title_version_when_enabled() {
        let item = SOURCE
            .details(json!({"manga": "/manga/sample", "preferences": {"removeTitleVersion": true}}))
            .unwrap();
        assert_eq!(item.title, "Sample Manga");
    }
}
