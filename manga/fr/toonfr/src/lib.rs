use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{
    html,
    manga::{self, MadaraConfig},
    sdk::SearchRequest,
};
use serde_json::Value;

const SOURCE: Source = Source;
const CONFIG: MadaraConfig = MadaraConfig {
    base_url: "https://toonfr.com",
    lang: "fr",
    content_rating: "adult",
    manga_path: "webtoon",
    popular_url_marker: "post-title",
    use_load_more: false,
    latest_enabled: true,
};

struct Source;

impl MangaSource for Source {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
        };
        Ok(parse_listing(&manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.list_url(page, order),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = deeplink(query) {
            let body = manga::Madara::fetch_document_or_fixture(
                &CONFIG,
                &CONFIG.absolute_url(&key),
                DETAILS_FIXTURE,
            );
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.search_url(page, query),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/webtoon/sample".into());
        let body =
            manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/webtoon/sample".into());
        let detail_url = CONFIG.absolute_url(&key);
        let detail_body = manga::Madara::fetch_document_or_fixture(&CONFIG, &detail_url, DETAILS_FIXTURE);
        let ajax = manga::Madara::browser_client(&CONFIG)
            .post(format!(
                "{}/ajax/chapters/",
                detail_url.trim_end_matches('/')
            ))
            .form(&[])
            .xhr()
            .send_text()
            .unwrap_or_else(|_| detail_body.clone());
        let chapters = manga::Madara::parse_chapters(&ajax, &key, &CONFIG);
        if chapters.len() == 1 && chapters[0].key == key {
            Ok(manga::Madara::parse_chapters(&detail_body, &key, &CONFIG))
        } else {
            Ok(chapters)
        }
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/webtoon/sample/chapter-1".into());
        let body =
            manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), PAGES_FIXTURE);
        Ok(manga::Madara::parse_pages(&body, &CONFIG))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| CONFIG.absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| CONFIG.absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = deeplink(input) {
            let body = manga::Madara::fetch_document_or_fixture(
                &CONFIG,
                &CONFIG.absolute_url(&key),
                DETAILS_FIXTURE,
            );
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: manga::Madara::parse_listing(body, &CONFIG),
        has_next_page: manga::Madara::has_next_page(body, &CONFIG),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let mut item = manga::Madara::parse_details(body, key, &CONFIG);
    item.title = html::text_between(body, "post-content", "</h3>")
        .or_else(|| html::text_between(body, "<h3", "</h3>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or(item.title);
    if let Some(alt) = info_value(body, "Autre nom").filter(|value| !value.is_empty()) {
        item.description = Some(match item.description.take() {
            Some(description) if !description.is_empty() => {
                format!("{description}\n\nAutre nom: {alt}")
            }
            _ => format!("Autre nom: {alt}"),
        });
    }
    item.status = info_value(body, "Statut").map_or(item.status, |value| parse_status(&value));
    item
}

fn info_value(body: &str, label: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let label_lower = label.to_ascii_lowercase();
    let index = lower.find(&label_lower)?;
    let fragment = &body[index..body.len().min(index + 800)];
    html::text_between(fragment, "summary-content", "</")
        .or_else(|| html::text_between(fragment, "<span", "</span>"))
        .or_else(|| html::text_between(fragment, "<a", "</a>"))
        .map(|value| html::strip_tags(&value))
        .map(|value| value.trim_matches([':', ' ', '\n', '\t']).to_string())
        .filter(|value| !value.is_empty() && value != "-" && value != "N/A")
}

fn parse_status(value: &str) -> ItemStatus {
    let value = value.to_ascii_lowercase();
    if value.contains("en cours") || value.contains("ongoing") {
        ItemStatus::Ongoing
    } else if value.contains("terminé") || value.contains("termine") || value.contains("completed")
    {
        ItemStatus::Completed
    } else if value.contains("pause") || value.contains("hiatus") {
        ItemStatus::Hiatus
    } else if value.contains("abandonné") || value.contains("abandonne") {
        ItemStatus::Cancelled
    } else {
        ItemStatus::Unknown
    }
}

fn deeplink(input: &str) -> Option<String> {
    (input.starts_with(CONFIG.base_url) && input.contains("/webtoon/"))
        .then(|| CONFIG.normalize_manga_key(input))
}

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><div class="item-thumb"><a href="https://toonfr.com/webtoon/sample/"><img src="/cover.jpg"></a></div><div class="post-title"><h3><a href="https://toonfr.com/webtoon/sample/">Sample</a></h3></div></div>
<div class="nav-previous"><a href="/webtoon/page/2/?m_orderby=views">Next</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="post-content"><h3>Sample</h3></div><div class="summary_image"><img src="/cover.jpg"></div>
<div class="description-summary"><p>Resume</p></div><div class="post-content_item"><div class="summary-heading">Autre nom</div><div class="summary-content">Alt</div></div>
<div class="summary-heading">Statut</div><div class="summary-content">En cours</div>
<ul><li class="wp-manga-chapter"><a href="https://toonfr.com/webtoon/sample/chapter-1/">Chapitre 1</a><span class="chapter-release-date">2024-01-01</span></li></ul>
"#;

const PAGES_FIXTURE: &str = r#"<div class="reading-content"><img class="wp-manga-chapter-img" data-src="/page1.jpg"><img class="wp-manga-chapter-img" src="/page2.jpg"></div>"#;
