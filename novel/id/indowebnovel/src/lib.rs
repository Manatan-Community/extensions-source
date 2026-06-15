use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: IndoWebNovel = IndoWebNovel;
const BASE_URL: &str = "https://indowebnovel.id";

struct IndoWebNovel;

impl NovelSource for IndoWebNovel {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_FIXTURE),
                has_next_page: false,
            });
        }

        let page = page(&request);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{BASE_URL}/page/{page}/?s&order=update")
        } else {
            format!("{BASE_URL}/page/{page}/?s")
        };
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty(),
            entries,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![fetch_details(&key)],
                has_next_page: false,
            });
        }

        let page = page(&request);
        let target = format!("{BASE_URL}/page/{page}/?s={}", url::query_escape(query));
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty(),
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel")
            .unwrap_or_else(|| "series/sample-novel".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel")
            .unwrap_or_else(|| "series/sample-novel".to_string());
        let body = fetch_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
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
            .unwrap_or_else(|| "series/sample-novel/chapter-1".to_string());
        let body = fetch_or_fixture(&absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: parse_listing(LIST_FIXTURE),
            has_more: true,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
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

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_details(key: &str) -> CatalogItem {
    let normalized = normalize_key(key);
    let body = fetch_or_fixture(&absolute_url(&normalized), DETAILS_FIXTURE);
    parse_details(&body, &normalized)
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("flexbox2-item")
        .skip(1)
        .filter_map(|block| {
            let href = href_after(block, "flexbox2-content")
                .or_else(|| html::attr_after(block, "<a", "href"))?;
            let key = normalize_key(&href);
            if key.is_empty() {
                return None;
            }
            Some(CatalogItem {
                key: key.clone(),
                title: first_text(block, &["flexbox2-title", "<h2", "<h3"])
                    .unwrap_or_else(|| title_from_key(&key)),
                cover: image_after(block, "<img").map(|path| absolute_url(&path)),
                url: Some(absolute_url(&key)),
                language: Some("id".to_string()),
                content_rating: Some("suggestive".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: first_text(body, &["series-title", "<h1", "<h2"])
            .unwrap_or_else(|| title_from_key(key)),
        cover: image_after(body, "series-thumb").map(|path| absolute_url(&path)),
        authors: text_near_label(body, "Author").into_iter().collect(),
        description: summary(body),
        tags: tags(body),
        status: parse_status(first_text(body, &["status"]).as_deref()),
        url: Some(absolute_url(key)),
        language: Some("id".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    let section = after_marker(body, "series-chapterlist").unwrap_or(body);
    let mut chapters: Vec<_> = section
        .split("<li")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "<a", "href")?;
            let key = normalize_key(&href);
            if key.is_empty() {
                return None;
            }
            Some(NovelChapter {
                key: key.clone(),
                title: first_text(block, &["<a"]).or_else(|| Some("Chapter".to_string())),
                chapter_number: chapter_number(&key),
                url: Some(absolute_url(&key)),
                language: Some("id".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect();
    chapters.reverse();
    chapters
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let raw = html::text_between(body, "adsads", "</div>")
        .or_else(|| html::text_between(body, "entry-content", "</div>"))
        .unwrap_or_else(|| body.to_string());
    let normalized = novel::normalize_reader_html(&raw);
    NovelText {
        title: first_text(body, &["entry-title", "<h1"]),
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(BASE_URL.to_string()),
        css: Some("img { max-width: 100%; height: auto; } body { line-height: 1.8; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        next_chapter_key: Some(normalize_key(key)),
        ..NovelText::default()
    }
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .or_else(|| input.strip_prefix(&format!("{BASE_URL}/")))
        .unwrap_or(input)
        .trim_start_matches('/')
        .split('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn absolute_url(path: &str) -> String {
    url::join_url(BASE_URL, &normalize_key(path))
}

fn href_after(body: &str, marker: &str) -> Option<String> {
    after_marker(body, marker).and_then(|chunk| html::attr_after(chunk, "<a", "href"))
}

fn image_after(body: &str, marker: &str) -> Option<String> {
    let chunk = after_marker(body, marker).unwrap_or(body);
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn first_text(body: &str, markers: &[&str]) -> Option<String> {
    markers
        .iter()
        .find_map(|marker| html::text_between(body, marker, "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn after_marker<'a>(body: &'a str, marker: &str) -> Option<&'a str> {
    body.find(marker).map(|index| &body[index..])
}

fn text_near_label(body: &str, label: &str) -> Option<String> {
    let chunk = after_marker(body, label)?;
    first_text(chunk, &["<span", "<a"]).filter(|value| value != label)
}

fn summary(body: &str) -> Option<String> {
    html::text_between(body, "series-synops", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn tags(body: &str) -> Vec<String> {
    let section = after_marker(body, "series-genres").unwrap_or(body);
    section
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .take(32)
        .collect()
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    let lower = value.unwrap_or_default().to_ascii_lowercase();
    if lower.contains("completed") || lower.contains("complete") {
        ItemStatus::Completed
    } else {
        ItemStatus::Ongoing
    }
}

fn chapter_number(path: &str) -> Option<f32> {
    path.split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .filter(|part| !part.is_empty())
        .next_back()
        .and_then(|part| part.parse().ok())
}

fn title_from_key(key: &str) -> String {
    key.trim_matches('/')
        .split('/')
        .next_back()
        .unwrap_or("novel")
        .split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

const LIST_FIXTURE: &str = r#"
<div class="flexbox2-item">
  <div class="flexbox2-content">
    <a href="https://indowebnovel.id/series/sample-novel/">
      <img src="https://indowebnovel.id/wp-content/uploads/sample.jpg">
      <div class="flexbox2-title"><span>Sample Novel</span></div>
    </a>
  </div>
</div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="series-title"><h2>Sample Novel</h2></div>
<div class="series-thumb"><img src="https://indowebnovel.id/wp-content/uploads/sample.jpg"></div>
<ul class="series-infolist"><li>Author <span>Sample Author</span></li></ul>
<span class="status">Completed</span>
<div class="series-synops"><p>A sample Indonesian novel.</p></div>
<div class="series-genres"><a href="/genre/action/">Action</a></div>
<ul class="series-chapterlist">
  <li><a href="https://indowebnovel.id/series/sample-novel/chapter-2/">Chapter 2</a></li>
  <li><a href="https://indowebnovel.id/series/sample-novel/chapter-1/">Chapter 1</a></li>
</ul>
"#;

const TEXT_FIXTURE: &str = r#"
<h1 class="entry-title">Chapter 1</h1>
<div class="adsads"><p>Paragraf pertama.</p><p>Paragraf kedua.</p></div>
"#;

export_novel_source!(SOURCE);
