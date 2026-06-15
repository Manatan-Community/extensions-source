use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: TimelessToons = TimelessToons;
const BASE_URL: &str = "https://timelesstoons.org";

struct TimelessToons;

impl MangaSource for TimelessToons {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            format!("{BASE_URL}/latest/")
        } else {
            BASE_URL.to_string()
        };
        let body = fetch_document(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_document(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let body = fetch_document(&search_url(query, request.get("filters")), SEARCH_FIXTURE);
        let mut entries = parse_cards(&body);
        if !query.is_empty() {
            let lower = query.to_ascii_lowercase();
            entries.retain(|entry| entry.title.to_ascii_lowercase().contains(&lower));
        }
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_cards(&fetch_document(BASE_URL, LIST_FIXTURE));
        let latest = parse_cards(&fetch_document(
            &format!("{BASE_URL}/latest/"),
            LIST_FIXTURE,
        ));
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular,
                has_more: false,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest,
                has_more: false,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(normalize_key(input)),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
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

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split(['<'])
        .filter(|part| part.contains("href=") && part.contains("/series/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::attr(chunk, "title")
                .filter(|value| !value.is_empty())
                .or_else(|| url::slug_from_url(&key))
                .unwrap_or_else(|| "Series".into());
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_from_style(chunk)
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample".into());
    let mut tags = link_texts(body, "genre=");
    tags.extend(detail_text(body, "Type").into_iter());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Series".into()),
        cover: image_from_style(body).map(|image| url::join_url(BASE_URL, &image)),
        description: detail_text(body, "Synopsis"),
        authors: detail_text(body, "Author").into_iter().collect(),
        artists: detail_text(body, "Artist").into_iter().collect(),
        tags,
        status: parse_status(&detail_text(body, "Status").unwrap_or_default()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("href=") && !chunk.contains("Upcoming"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains("/series/") {
                return None;
            }
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "text-sm", "</")
                    .or_else(|| html::text_between(chunk, ">", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| url::slug_from_url(&key)),
                date_uploaded: html::text_between(chunk, "text-xs", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                is_locked: chunk.contains("alt=\"Coin\"") || chunk.contains("alt='Coin'"),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".into()),
                ..MangaChapter::default()
            })
        })
        .filter(|chapter| !chapter.is_locked)
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let cdn = cdn_url(body);
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("uid=") || chunk.contains("cdn") || chunk.contains("keyoapp")
        })
        .filter_map(|chunk| {
            html::attr(chunk, "uid")
                .and_then(|uid| cdn.as_ref().map(|base| format!("{base}/{uid}")))
                .or_else(|| html::attr(chunk, "data-src"))
                .or_else(|| html::attr(chunk, "data-lazy-src"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn search_url(query: &str, filters: Option<&Value>) -> String {
    let mut params = Vec::new();
    if !query.is_empty() {
        params.push(("q", query.to_string()));
    }
    for key in ["genre", "type", "status"] {
        for value in filter_values(filters, key) {
            params.push((key, value));
        }
    }
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{BASE_URL}/series/?{query}")
}

fn filter_values(filters: Option<&Value>, key: &str) -> Vec<String> {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn detail_text(body: &str, label: &str) -> Option<String> {
    body.split("<span")
        .find(|chunk| html::strip_tags(chunk).trim().eq_ignore_ascii_case(label))
        .and_then(|chunk| html::text_between(chunk, "</span>", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| {
            body.split(label)
                .nth(1)
                .and_then(|rest| html::text_between(rest, "<div", "</div>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
        })
}

fn link_texts(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .fold(Vec::new(), |mut values, value| {
            if !values.contains(&value) {
                values.push(value);
            }
            values
        })
}

fn image_from_style(body: &str) -> Option<String> {
    let style =
        html::attr_after(body, "photoURL", "style").or_else(|| html::attr(body, "style"))?;
    let marker = "url(";
    let start = style.find(marker)? + marker.len();
    let rest = &style[start..];
    Some(
        rest.split(')')
            .next()
            .unwrap_or_default()
            .trim_matches(['"', '\''])
            .to_string(),
    )
    .filter(|value| !value.is_empty())
}

fn cdn_url(body: &str) -> Option<String> {
    let marker = "realUrl";
    let chunk = body.split(marker).nth(1)?;
    let host = chunk
        .split("//")
        .nth(1)?
        .split(['/', '`', '"', '\''])
        .next()?
        .replace("${cdn}", "");
    (!host.is_empty()).then(|| format!("https://{host}/uploads"))
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "dropped" => ItemStatus::Cancelled,
        "paused" | "hiatus" => ItemStatus::Hiatus,
        "completed" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn normalize_key(value: &str) -> String {
    if let Some(path) = value.strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut values: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !values.iter().any(|existing| existing.key == item.key) {
        values.push(item);
    }
    values
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="group"><a href="/series/sample" title="Sample Toon" tags="[&quot;action&quot;]" data-type="manga" data-status="ongoing" style="background-image:url('/cover.jpg')"></a></div>"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"
<div class="grid"><h1>Sample Toon</h1><div class="photoURL" style="background-image:url('/cover.jpg')"></div><span>Status</span><div>ongoing</div><span>Author</span><div>Author</div><span>Artist</span><div>Artist</div><span>Type</span><div>manga</div><a href="/series/?genre=action">Action</a><div>Synopsis</div><div>Sample summary.</div></div>
<div id="chapters"><a href="/series/sample/chapter-1"><span class="text-sm">Chapter 1</span><span class="text-xs">Jan 1, 2024</span></a></div>
"#;
const PAGES_FIXTURE: &str = r#"<script>realUrl = `https://cdn.keyoapp.com`</script><div id="pages"><img uid="sample.jpg"></div>"#;
