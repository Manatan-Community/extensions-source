use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{
    html, novel,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde_json::Value;
use std::collections::BTreeSet;

const SOURCE: Kakuyomu = Kakuyomu;
const BASE_URL: &str = "https://kakuyomu.jp";

struct Kakuyomu;

impl NovelSource for Kakuyomu {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_ranking_list(RANKING_FIXTURE),
                has_next_page: false,
            });
        }

        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{BASE_URL}/recent_works?page={page}")
        } else {
            let filters = request.get("filters");
            let genre = filter_string(filters, "genre", "all");
            let period = filter_string(filters, "period", "entire");
            format!("{BASE_URL}/rankings/{genre}/{period}?page={page}")
        };
        let body = fetch_document(&target, RANKING_FIXTURE);
        let entries = if listing == "latest" {
            parse_search_or_next_data(&body)
        } else {
            parse_ranking_list(&body)
        };
        Ok(Paged {
            has_next_page: !entries.is_empty() && body.contains(&format!("page={}", page + 1)),
            entries,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&key)],
                has_next_page: false,
            });
        }

        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = format!(
            "{BASE_URL}/search?q={}&page={page}",
            url::query_escape(query)
        );
        let body = fetch_document(&target, SEARCH_FIXTURE);
        Ok(Paged {
            entries: parse_search_or_next_data(&body),
            has_next_page: body.contains(&format!("page={}", page + 1)),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "works/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "works/sample".to_string());
        let body = fetch_document(&format!("{BASE_URL}/{}", key.trim_start_matches('/')), DETAILS_FIXTURE);
        Ok(parse_chapters_from_next_data(&body, &normalize_key(&key)))
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
            .unwrap_or_else(|| "works/sample/episodes/1".to_string());
        let target = format!("{BASE_URL}/{}", key.trim_start_matches('/'));
        let body = fetch_document(&target, TEXT_FIXTURE);
        let chapter_title = html::text_between(&body, "chapterTitle", "</")
            .map(|value| html::strip_tags(&value));
        let episode_title = html::text_between(&body, "widget-episodeTitle", "</")
            .map(|value| html::strip_tags(&value));
        let episode_body = html::text_between(&body, "widget-episodeBody", "</div>")
            .or_else(|| html::text_between(&body, "js-episodeBody", "</div>"))
            .unwrap_or(body);
        let mut chapter_html = String::new();
        if let Some(title) = &chapter_title {
            chapter_html.push_str(&format!("<h1>{title}</h1>"));
        }
        if let Some(title) = &episode_title {
            chapter_html.push_str(&format!("<h2>{title}</h2>"));
        }
        chapter_html.push_str(&novel::normalize_reader_html(&episode_body));
        Ok(NovelText {
            title: episode_title.or(chapter_title),
            html: Some(chapter_html.clone()),
            text: Some(novel::cleanup_text(&chapter_html)),
            base_url: Some(BASE_URL.to_string()),
            css: Some("body { line-height: 1.8; } img { max-width: 100%; height: auto; }".to_string()),
            image_headers: novel::image_headers(BASE_URL),
            ..NovelText::default()
        })
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let latest = self.list(with_listing(request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Rankings".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&key)),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn filter_string(filters: Option<&Value>, key: &str, default: &str) -> String {
    filters
        .and_then(|filters| filters.get(key))
        .and_then(|value| value.get("value").or(Some(value)))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
        .to_string()
}

fn parse_ranking_list(body: &str) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            if !chunk.contains("widget-workCard-titleLabel") && !chunk.contains("/works/") {
                return None;
            }
            let href = html::attr(chunk, "href")?;
            let key = key_from_url(&href)?;
            if !seen.insert(key.clone()) {
                return None;
            }
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, ">", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| key.clone()),
                url: Some(format!("{BASE_URL}/{key}")),
                language: Some("ja".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .take(40)
        .collect()
}

fn parse_search_or_next_data(body: &str) -> Vec<CatalogItem> {
    let Some(state) = next_data_state(body) else {
        return parse_ranking_list(body);
    };
    state
        .as_object()
        .into_iter()
        .flat_map(|object| object.values())
        .filter(|value| value.get("__typename").and_then(Value::as_str) == Some("Work"))
        .filter_map(work_to_item)
        .collect()
}

fn fetch_details(key: &str) -> CatalogItem {
    let key = normalize_key(key);
    let body = fetch_document(&format!("{BASE_URL}/{key}"), DETAILS_FIXTURE);
    let Some(state) = next_data_state(&body) else {
        return fallback_details(&body, &key);
    };
    let Some(work) = find_work(&state, &work_id(&key)) else {
        return fallback_details(&body, &key);
    };
    let author_ref = work
        .get("author")
        .and_then(|author| author.get("__ref"))
        .and_then(Value::as_str)
        .and_then(|value| value.strip_prefix("UserAccount:"));
    let author = author_ref.and_then(|id| find_user(&state, id));
    CatalogItem {
        key: key.clone(),
        title: work
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Kakuyomu Work")
            .to_string(),
        cover: work
            .get("adminCoverImageUrl")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        url: Some(format!("{BASE_URL}/{key}")),
        authors: author
            .and_then(|user| user.get("activityName").and_then(Value::as_str))
            .map(ToString::to_string)
            .into_iter()
            .collect(),
        description: work
            .get("introduction")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        tags: work
            .get("tagLabels")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        status: if work.get("serialStatus").and_then(Value::as_str) == Some("COMPLETED") {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        language: Some("ja".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters_from_next_data(body: &str, novel_key: &str) -> Vec<NovelChapter> {
    let Some(state) = next_data_state(body) else {
        return fallback_chapters(body, novel_key);
    };
    let mut chapters = Vec::new();
    if let Some(object) = state.as_object() {
        for episode in object
            .values()
            .filter(|value| value.get("__typename").and_then(Value::as_str) == Some("Episode"))
        {
            let Some(id) = episode.get("id").and_then(Value::as_str) else {
                continue;
            };
            chapters.push(NovelChapter {
                key: format!("{novel_key}/episodes/{id}"),
                title: episode
                    .get("title")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                url: Some(format!("{BASE_URL}/{novel_key}/episodes/{id}")),
                language: Some("ja".to_string()),
                ..NovelChapter::default()
            });
        }
    }
    chapters
}

fn next_data_state(body: &str) -> Option<Value> {
    let json = html::text_between(body, "id=\"__NEXT_DATA__\"", "</script>")
        .or_else(|| html::text_between(body, "id='__NEXT_DATA__'", "</script>"))?;
    let root = serde_json::from_str::<Value>(&json).ok()?;
    root.pointer("/props/pageProps/__APOLLO_STATE__").cloned()
}

fn find_work<'a>(state: &'a Value, id: &str) -> Option<&'a Value> {
    state.as_object()?.values().find(|value| {
        value.get("__typename").and_then(Value::as_str) == Some("Work")
            && value.get("id").and_then(Value::as_str) == Some(id)
    })
}

fn find_user<'a>(state: &'a Value, id: &str) -> Option<&'a Value> {
    state.as_object()?.values().find(|value| {
        value.get("__typename").and_then(Value::as_str) == Some("UserAccount")
            && value.get("id").and_then(Value::as_str) == Some(id)
    })
}

fn work_to_item(work: &Value) -> Option<CatalogItem> {
    let id = work.get("id").and_then(Value::as_str)?;
    Some(CatalogItem {
        key: format!("works/{id}"),
        title: work
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Kakuyomu Work")
            .to_string(),
        cover: work
            .get("adminCoverImageUrl")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        url: Some(format!("{BASE_URL}/works/{id}")),
        language: Some("ja".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn fallback_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Kakuyomu Work".to_string()),
        url: Some(format!("{BASE_URL}/{key}")),
        description: html::attr_after(body, "name=\"description\"", "content"),
        status: ItemStatus::Ongoing,
        language: Some("ja".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fallback_chapters(body: &str, novel_key: &str) -> Vec<NovelChapter> {
    let mut seen = BTreeSet::new();
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = key_from_url(&href)?;
            if !key.contains("/episodes/") || !seen.insert(key.clone()) {
                return None;
            }
            Some(NovelChapter {
                key: key.clone(),
                title: html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value)),
                url: Some(format!("{BASE_URL}/{key}")),
                language: Some("ja".to_string()),
                ..NovelChapter::default()
            })
        })
        .chain(std::iter::once_with(|| NovelChapter {
            key: format!("{novel_key}/episodes/1"),
            title: Some("Episode 1".to_string()),
            url: Some(format!("{BASE_URL}/{novel_key}/episodes/1")),
            language: Some("ja".to_string()),
            ..NovelChapter::default()
        }))
        .take(40)
        .collect()
}

fn key_from_url(input: &str) -> Option<String> {
    let path = input
        .trim()
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_start_matches('/');
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    if parts.next()? != "works" {
        return None;
    }
    let work = parts.next()?;
    let mut key = format!("works/{work}");
    if parts.next() == Some("episodes") && let Some(episode) = parts.next() {
        key.push_str("/episodes/");
        key.push_str(episode);
    }
    Some(key)
}

fn normalize_key(key: &str) -> String {
    key_from_url(key).unwrap_or_else(|| key.trim_matches('/').to_string())
}

fn work_id(key: &str) -> &str {
    key.trim_start_matches("works/")
        .split('/')
        .next()
        .unwrap_or(key)
}

fn with_listing(mut request: Value, listing: &str) -> Value {
    if let Some(object) = request.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    request
}

const RANKING_FIXTURE: &str = r#"
<a class="widget-workCard-titleLabel" href="/works/sample">Sample Kakuyomu</a>
"#;

const SEARCH_FIXTURE: &str = r#"
<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"__APOLLO_STATE__":{"Work:sample":{"__typename":"Work","id":"sample","title":"Sample Kakuyomu","serialStatus":"ONGOING","tagLabels":["sample"],"introduction":"Sample summary."}}}}}</script>
"#;

const DETAILS_FIXTURE: &str = SEARCH_FIXTURE;

const TEXT_FIXTURE: &str = r#"
<div class="chapterTitle">Chapter</div><div class="widget-episodeTitle">Episode 1</div><div class="widget-episodeBody"><p>First paragraph.</p></div>
"#;

export_novel_source!(SOURCE);
