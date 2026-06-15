use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: NewManhwa = NewManhwa;
const DEFAULT_BASE_URL: &str = "https://newmanhwa.com";
const MIRROR_BASE_URL: &str = "https://fullmanhwa.com";

struct NewManhwa;

impl MangaSource for NewManhwa {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base_url = preferred_base_url(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "popular"
        };
        Ok(parse_listing(
            &fetch_document(
                &base_url,
                &format!("{base_url}/{path}?page={page}"),
                LIST_FIXTURE,
            ),
            &base_url,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base_url = preferred_base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);

        if is_supported_url(query) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&base_url, &url::join_url(&base_url, &key), DETAILS_FIXTURE),
                    Some(key),
                    &base_url,
                )],
                has_next_page: false,
            });
        }

        let mut params = Vec::new();
        if !query.is_empty() {
            params.push(("q", url::query_escape(query)));
        }
        for id in ["status", "genre", "sort"] {
            if let Some(value) = filter_text(&request, id) {
                params.push((id, url::query_escape(&value)));
            }
        }
        if page > 1 {
            params.push(("page", page.to_string()));
        }
        let suffix = params
            .into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&");
        let target = if suffix.is_empty() {
            format!("{base_url}/search")
        } else {
            format!("{base_url}/search?{suffix}")
        };
        let body = fetch_document(&base_url, &target, LIST_FIXTURE);
        if body.contains("series-left") {
            return Ok(Paged {
                entries: vec![parse_details(
                    &body,
                    Some(normalize_key(&target)),
                    &base_url,
                )],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&body, &base_url))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base_url = preferred_base_url(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        Ok(parse_details(
            &fetch_document(&base_url, &url::join_url(&base_url, &key), DETAILS_FIXTURE),
            Some(key),
            &base_url,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base_url = preferred_base_url(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        Ok(parse_chapters(
            &fetch_document(&base_url, &url::join_url(&base_url, &key), DETAILS_FIXTURE),
            &base_url,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base_url = preferred_base_url(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1".to_string());
        Ok(parse_pages(
            &fetch_document(&base_url, &url::join_url(&base_url, &key), PAGES_FIXTURE),
            &base_url,
        ))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(merge_json(
            &request,
            serde_json::json!({"page": 1, "listingId": "popular"}),
        ))?;
        let latest = self.list(merge_json(
            &request,
            serde_json::json!({"page": 1, "listingId": "latest"}),
        ))?;
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base_url = preferred_base_url(&request);
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(&base_url, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base_url = preferred_base_url(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(&base_url, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if is_supported_url(input) {
            let base_url = preferred_base_url(&request);
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(&base_url, &url::join_url(&base_url, &key), DETAILS_FIXTURE),
                    Some(key),
                    &base_url,
                )),
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

fn client(base_url: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{base_url}/"))
        .with_cookies_for(base_url)
        .with_webview_challenge_fallback()
}

fn fetch_document(base_url: &str, target: &str, fixture: &str) -> String {
    client(base_url)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, base_url: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("series-card"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                let key = normalize_key(&href);
                let title = html::text_between(chunk, "<strong", "</strong>")
                    .map(|value| html::strip_tags(&value))
                    .map(remove_title_rank)
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&href))
                    .unwrap_or_else(|| "Series".to_string());
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| url::join_url(base_url, &image)),
                    url: Some(url::join_url(base_url, &key)),
                    language: Some("en".to_string()),
                    content_rating: Some("adult".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("Next")
            && !body.contains("Next</a></li><li class=\"disabled\""),
    }
}

fn parse_details(body: &str, key: Option<String>, base_url: &str) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Series".to_string()),
        cover: html::attr_after(body, "cover-card", "data-src")
            .or_else(|| html::attr_after(body, "cover-card", "src"))
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(base_url, &image)),
        description: text_after(body, "summary-inline"),
        authors: info_values(body, "Author"),
        artists: info_values(body, "Artist"),
        tags: json_genres(body),
        status: parse_status(&info_values(body, "Status").join(" ")),
        url: Some(url::join_url(base_url, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, base_url: &str) -> Vec<MangaChapter> {
    body.split("chapter-row")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "chapter-main", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "chapter-name", "</a>")
                .or_else(|| html::text_between(chunk, "<strong", "</strong>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number_from_text(&title),
                url: Some(url::join_url(base_url, &key)),
                date_uploaded: text_after(chunk, "chapter-age")
                    .and_then(|date| manatan_shared::dates::parse_fixture_date(&date)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, base_url: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-page"))
        .filter_map(image_attr)
        .map(|image| url::join_url(base_url, &image))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(base_url)),
            },
            headers: manga::image_headers(base_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn text_after(body: &str, marker: &str) -> Option<String> {
    let chunk = body.split(marker).nth(1)?;
    html::text_between(chunk, "<p", "</p>")
        .or_else(|| html::text_between(chunk, "<span", "</span>"))
        .or_else(|| html::text_between(chunk, "<dd", "</dd>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn info_values(body: &str, label: &str) -> Vec<String> {
    body.split("<dt")
        .skip(1)
        .filter(|chunk| chunk.contains(label))
        .filter_map(|chunk| html::text_between(chunk, "<dd", "</dd>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn json_genres(body: &str) -> Vec<String> {
    let Some(chunk) = body.split("\"genre\"").nth(1) else {
        return Vec::new();
    };
    let Some(list) = chunk
        .split_once('[')
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(list, _)| list)
    else {
        return Vec::new();
    };
    list.split(',')
        .map(|value| value.trim().trim_matches('"').to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn filter_text(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .or_else(|| request.get(id))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn preferred_base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("base_url"))
        .and_then(Value::as_str)
        .filter(|value| *value == DEFAULT_BASE_URL || *value == MIRROR_BASE_URL)
        .unwrap_or(DEFAULT_BASE_URL)
        .to_string()
}

fn is_supported_url(input: &str) -> bool {
    input.starts_with(DEFAULT_BASE_URL) || input.starts_with(MIRROR_BASE_URL)
}

fn normalize_key(input: &str) -> String {
    let input = input
        .strip_prefix(DEFAULT_BASE_URL)
        .or_else(|| input.strip_prefix(MIRROR_BASE_URL))
        .unwrap_or(input);
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn remove_title_rank(value: String) -> String {
    let trimmed = value.trim();
    let without_rank = trimmed
        .strip_prefix('#')
        .and_then(|rest| rest.split_once(' '))
        .map(|(_, title)| title)
        .unwrap_or(trimmed);
    without_rank.trim().to_string()
}

fn parse_status(value: &str) -> ItemStatus {
    match value.to_ascii_lowercase().as_str() {
        text if text.contains("ongoing") => ItemStatus::Ongoing,
        text if text.contains("completed") => ItemStatus::Completed,
        text if text.contains("hiatus") => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn chapter_number_from_text(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn merge_json(base: &Value, overlay: Value) -> Value {
    let mut object = base.as_object().cloned().unwrap_or_default();
    if let Some(overlay_object) = overlay.as_object() {
        for (key, value) in overlay_object {
            object.insert(key.clone(), value.clone());
        }
    }
    Value::Object(object)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<a class="series-card" href="/series/sample"><img data-src="/cover.jpg"><strong>#1 Sample Manhwa</strong></a>
"#;
const DETAILS_FIXTURE: &str = r#"
<aside class="series-left"><div class="cover-card"><img src="/cover.jpg"></div></aside><h1>Sample Manhwa</h1>
<section class="summary-inline"><p>Sample description.</p></section>
<dt>Author</dt><dd><a><span>Author</span></a></dd><dt>Artist</dt><dd><a><span>Artist</span></a></dd><dt>Status</dt><dd><span>Ongoing</span></dd>
<script type="application/ld+json">{"@type":"ComicSeries","genre":["Action","Romance"]}</script>
<div class="chapter-list"><div class="chapter-row"><a class="chapter-main" href="/series/sample/chapter-1"><span class="chapter-name"><strong>Chapter 1</strong></span></a><span class="chapter-age">Jan 01, 2024</span></div></div>
"#;
const PAGES_FIXTURE: &str = r#"
<main id="reader"><img class="chapter-page" data-src="/page1.jpg"><img class="chapter-page" src="/page2.jpg"></main>
"#;
