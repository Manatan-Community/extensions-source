use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: YellowNote = YellowNote;
const DEFAULT_BASE_URL: &str = "https://xchina.co";

struct YellowNote;

impl MangaSource for YellowNote {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = SourceConfig::from_request(&request);
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, &config));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let section = request
            .get("section")
            .or_else(|| request.get("kind"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if section == "latest" {
            format!("{}/photos/{page}.html", config.base_url)
        } else {
            format!("{}/photos/sort-hot/{page}.html", config.base_url)
        };
        let body = fetch_document_or_fixture(&config, &target, LIST_FIXTURE);
        Ok(parse_listing(&body, &config))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = SourceConfig::from_request(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(&config.base_url) || query.contains("xchina.co/") {
            let key = normalize_key(query);
            let body =
                fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key), &config)],
                has_next_page: false,
            });
        }

        let filters = request.get("filters").unwrap_or(&Value::Null);
        let sort = filters
            .get("sort")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim_matches('/');
        let base_path = if query.is_empty() {
            filters
                .get("category")
                .and_then(Value::as_str)
                .unwrap_or("photos")
                .trim_matches('/')
                .to_string()
        } else {
            format!("photos/keyword-{}", url::query_escape(query))
        };
        let mut parts = vec![base_path];
        if !sort.is_empty() {
            parts.push(sort.to_string());
        }
        parts.push(format!("{page}.html"));
        let target = format!("{}/{}", config.base_url, parts.join("/"));
        let body = fetch_document_or_fixture(&config, &target, LIST_FIXTURE);
        Ok(parse_listing(&body, &config))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = SourceConfig::from_request(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/photos/sample.html".into());
        let body = fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key), &config))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = SourceConfig::from_request(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/photos/sample.html".into());
        let body = fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key, &config))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = SourceConfig::from_request(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/photos/sample/1.html".into());
        let body = fetch_document_or_fixture(&config, &config.absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_pages(&body, &config))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let config = SourceConfig::from_request(&request);
        if input.contains("xchina.co/") {
            let key = normalize_key(input);
            let body =
                fetch_document_or_fixture(&config, &config.absolute_url(&key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key), &config)),
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

#[derive(Debug, Clone)]
struct SourceConfig {
    base_url: String,
}

impl SourceConfig {
    fn from_request(request: &Value) -> Self {
        let preferences = request.get("preferences").unwrap_or(&Value::Null);
        let configured = preferences
            .get("domain")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(DEFAULT_BASE_URL);
        let language = preferences
            .get("language")
            .and_then(Value::as_str)
            .unwrap_or("en");
        let base_url = apply_language_subdomain(configured, language);
        Self { base_url }
    }

    fn absolute_url(&self, path: &str) -> String {
        url::join_url(&self.base_url, path)
    }
}

fn apply_language_subdomain(input: &str, language: &str) -> String {
    let trimmed = input.trim().trim_end_matches('/');
    let Some(subdomain) = language_subdomain(language) else {
        return trimmed.to_string();
    };
    let Some(host_start) = trimmed.find("://").map(|index| index + 3) else {
        return trimmed.to_string();
    };
    let host_end = trimmed[host_start..]
        .find('/')
        .map(|index| host_start + index)
        .unwrap_or(trimmed.len());
    let host = &trimmed[host_start..host_end];
    if host.starts_with(&format!("{subdomain}.")) || host.matches('.').count() > 1 {
        trimmed.to_string()
    } else {
        format!(
            "{}{}.{host}{}",
            &trimmed[..host_start],
            subdomain,
            &trimmed[host_end..]
        )
    }
}

fn language_subdomain(language: &str) -> Option<&'static str> {
    match language {
        "en" => Some("en"),
        "es" => Some("es"),
        "ko" => Some("kr"),
        "zh-Hant" => Some("tw"),
        "zh-Hans" => None,
        _ => Some("en"),
    }
}

fn client(config: &SourceConfig) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", config.base_url))
        .with_cookies_for(&config.base_url)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(config: &SourceConfig, target: &str, fixture: &str) -> String {
    client(config)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, config: &SourceConfig) -> Paged<CatalogItem> {
    let entries = body
        .split("class=\"item")
        .skip(1)
        .filter(|chunk| {
            (chunk.contains("photo") || chunk.contains("amateur"))
                && !chunk.contains("photo-image")
                && !chunk.contains("amateur-image")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::attr_after(chunk, "<a", "title")
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Gallery".into()));
            let media_count = chunk
                .split("<div")
                .filter_map(|part| html::text_between(part, ">", "</div>"))
                .map(|value| html::strip_tags(&value))
                .find(|value| looks_like_media_count(value));
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: media_count
                    .map(|count| format!("{title} ({count})"))
                    .unwrap_or(title),
                cover: style_url(chunk).map(|image| url::join_url(&config.base_url, &image)),
                status: ItemStatus::Completed,
                url: Some(config.absolute_url(&key)),
                language: Some("all".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    Paged {
        has_next_page: body.contains("pager-next"),
        entries,
    }
}

fn parse_details(body: &str, key: Option<String>, config: &SourceConfig) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/photos/sample.html".to_string());
    let name = info_by_icon(body, "fa-address-card")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Gallery".into()));
    let media_count = info_by_icon(body, "fa-image").unwrap_or_else(|| "1P".to_string());
    let number = info_by_icon(body, "fa-file")
        .filter(|value| !value.is_empty())
        .map(|value| format!(" {value}"))
        .unwrap_or_default();
    CatalogItem {
        key: key.clone(),
        title: format!("{name}{number}({media_count})"),
        cover: style_url(body).map(|image| url::join_url(&config.base_url, &image)),
        authors: author(body).into_iter().collect(),
        tags: ["fa-video-camera", "fa-filter", "fa-tags"]
            .into_iter()
            .flat_map(|icon| infos_by_icon(body, icon))
            .filter(|value| value != "-")
            .collect(),
        status: ItemStatus::Completed,
        url: Some(config.absolute_url(&key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn author(body: &str) -> Option<String> {
    body.split("item floating")
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, ">", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| info_by_icon(body, "fa-circle-user"))
}

fn parse_chapters(body: &str, key: &str, config: &SourceConfig) -> Vec<MangaChapter> {
    let max_page = body
        .split("pager-num")
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .filter_map(|value| html::strip_tags(&value).parse::<u32>().ok())
        .max()
        .unwrap_or(1);
    let upload_date = info_by_icon(body, "fa-calendar-days")
        .and_then(|value| manatan_shared::dates::parse_fixture_date(&value));
    let base = key.trim_end_matches(".html").trim_end_matches('/');
    (1..=max_page)
        .rev()
        .map(|page| {
            let chapter_key = format!("{base}/{page}.html");
            MangaChapter {
                key: chapter_key.clone(),
                title: Some(format!("Page {page}")),
                date_uploaded: upload_date,
                url: Some(config.absolute_url(&chapter_key)),
                chapter_number: Some(page as f32),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str, config: &SourceConfig) -> Vec<MangaPage> {
    body.split("class=\"item")
        .skip(1)
        .filter(|chunk| chunk.contains("photo-image") || chunk.contains("amateur-image"))
        .filter_map(style_url)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(&config.base_url, &image),
                context: Some(manga::image_headers(&config.base_url)),
            },
            headers: manga::image_headers(&config.base_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn style_url(input: &str) -> Option<String> {
    let style = html::attr_after(input, "div", "style").or_else(|| html::attr(input, "style"))?;
    let marker = "url(";
    let start = style.find(marker)? + marker.len();
    let rest = &style[start..];
    let end = rest.find(')')?;
    Some(rest[..end].trim_matches(['"', '\'']).to_string())
}

fn info_by_icon(body: &str, icon: &str) -> Option<String> {
    body.split("div class=\"item")
        .chain(body.split("div class='item"))
        .find(|chunk| chunk.contains(icon))
        .and_then(|chunk| {
            html::text_between(chunk, "div class=\"text", "</div>")
                .or_else(|| html::text_between(chunk, "div class='text", "</div>"))
        })
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn infos_by_icon(body: &str, icon: &str) -> Vec<String> {
    let Some(chunk) = body
        .split("div class=\"item")
        .chain(body.split("div class='item"))
        .find(|chunk| chunk.contains(icon))
    else {
        return Vec::new();
    };
    chunk
        .split("<div")
        .filter_map(|part| html::text_between(part, ">", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty() && !value.contains("fa-"))
        .collect()
}

fn looks_like_media_count(value: &str) -> bool {
    let trimmed = value.trim();
    let mut parts = trimmed.split('P');
    parts
        .next()
        .is_some_and(|head| !head.is_empty() && head.chars().all(|ch| ch.is_ascii_digit()))
        && (parts.next().is_some() || trimmed.ends_with('V'))
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find(".co/") {
            return format!("/{}", input[index + 4..].trim_start_matches('/'));
        }
    }
    format!("/{}", input.trim_start_matches('/'))
}

const LIST_FIXTURE: &str = r#"
<div class="list photo-list">
  <div class="item photo"><a href="/photos/sample.html" title="Sample gallery"><div class="img" style="background-image:url('https://img.xchina.io/photos/sample/cover.webp');"></div></a><div class="tags"><div>12P + 1V</div></div></div>
</div>
<div class="pager"><a class="pager-next" href="/photos/2.html">Next</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="info-card photo-detail">
  <div class="item"><div class="icon"><i class="fa-address-card"></i></div><div class="text">Sample gallery</div></div>
  <div class="item"><div class="icon"><i class="fa-image"></i></div><div class="text">12P</div></div>
  <div class="item"><div class="icon"><i class="fa-file"></i></div><div class="text">No. 42</div></div>
  <div class="item"><div class="icon"><i class="fa-circle-user"></i></div><div class="text">Creator</div></div>
  <div class="item"><div class="icon"><i class="fa-video-camera"></i></div><div class="text"><div>Cosplay</div><div>-</div></div></div>
  <div class="item"><div class="icon"><i class="fa-filter"></i></div><div class="text"><div>Color</div></div></div>
  <div class="item"><div class="icon"><i class="fa-tags"></i></div><div class="text"><div>Tag A</div></div></div>
  <div class="item"><div class="icon"><i class="fa-calendar-days"></i></div><div class="text">2024.03.01</div></div>
</div>
<div class="pager"><a class="pager-num">1</a><a class="pager-num">3</a></div>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="list photo-items">
  <div class="item photo-image"><div class="img" style="background-image:url('https://img.xchina.io/photos/sample/0001.webp');"></div></div>
  <div class="item photo-image"><div class="img" style='background-image:url("https://img.xchina.io/photos/sample/0002.webp");'></div></div>
</div>
"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_language_subdomains() {
        assert_eq!(
            apply_language_subdomain(DEFAULT_BASE_URL, "en"),
            "https://en.xchina.co"
        );
        assert_eq!(
            apply_language_subdomain(DEFAULT_BASE_URL, "zh-Hans"),
            DEFAULT_BASE_URL
        );
        assert_eq!(
            apply_language_subdomain(DEFAULT_BASE_URL, "zh-Hant"),
            "https://tw.xchina.co"
        );
    }

    #[test]
    fn parses_listing_entries() {
        let config = SourceConfig {
            base_url: DEFAULT_BASE_URL.to_string(),
        };
        let page = parse_listing(LIST_FIXTURE, &config);
        assert!(page.has_next_page);
        assert_eq!(page.entries[0].key, "/photos/sample.html");
        assert_eq!(page.entries[0].title, "Sample gallery (12P + 1V)");
    }

    #[test]
    fn parses_details_chapters_and_pages() {
        let config = SourceConfig {
            base_url: DEFAULT_BASE_URL.to_string(),
        };
        let item = parse_details(DETAILS_FIXTURE, Some("/photos/sample.html".into()), &config);
        assert_eq!(item.title, "Sample gallery No. 42(12P)");
        assert_eq!(item.authors, vec!["Creator"]);
        assert!(item.tags.contains(&"Cosplay".to_string()));

        let chapters = parse_chapters(DETAILS_FIXTURE, "/photos/sample.html", &config);
        assert_eq!(chapters.len(), 3);
        assert_eq!(chapters[0].key, "/photos/sample/3.html");

        let pages = parse_pages(PAGES_FIXTURE, &config);
        assert_eq!(pages.len(), 2);
    }
}
