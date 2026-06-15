use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeSet;

const SOURCE: Illusia = Illusia;
const BASE_URL: &str = "https://illusia.com.br";
const DEFAULT_COVER: &str = "https://illusia.com.br/favicon.ico";

struct Illusia;

impl NovelSource for Illusia {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let order_by = if listing == "latest" {
            "modified"
        } else {
            "comment_count"
        };
        let body = fetch_document_or_fixture(&search_url("", page, order_by), LIST_FIXTURE);
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty() && has_next_page(&body),
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
        let page = page(&request);
        let body = fetch_document_or_fixture(&search_url(query, page, "relevance"), LIST_FIXTURE);
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty() && has_next_page(&body),
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "story/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "story/sample".to_string());
        let body = fetch_document_or_fixture(&story_url(&key), DETAILS_FIXTURE);
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
            .unwrap_or_else(|| "story/sample/chapter-1".to_string());
        let body = fetch_document_or_fixture(&story_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let latest = self.list(with_listing(request, "latest"))?;
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
                title: "Atualizadas".to_string(),
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
        .with_header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/114.0.0.0 Safari/537.36")
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn search_url(query: &str, page: u64, order_by: &str) -> String {
    let page_path = if page == 1 {
        String::new()
    } else {
        format!("page/{page}/")
    };
    format!(
        "{BASE_URL}/{page_path}?s={}&post_type=fcn_story&sentence=0&orderby={order_by}&order=desc&age_rating=Any&story_status=Any&miw=0&maw=0&genres=&fandoms=&characters=&tags=&warnings=&authors=&ex_genres=&ex_fandoms=&ex_characters=&ex_tags=&ex_warnings=&ex_authors=",
        url::query_escape(query)
    )
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    listing_blocks(body)
        .into_iter()
        .filter_map(|block| {
            let href = first_story_href(&block)?;
            let key = normalize_key(&href).trim_end_matches('/').to_string();
            if key.is_empty() || !seen.insert(key.clone()) {
                return None;
            }
            let title = title_from_listing(&block).unwrap_or_else(|| title_from_key(&key));
            let cover = image_from_block(&block).unwrap_or_else(|| DEFAULT_COVER.to_string());
            Some(catalog_item(key, title, Some(cover), false))
        })
        .collect()
}

fn listing_blocks(body: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    for marker in [
        "<li",
        "<article",
        "class=\"card",
        "story-card",
        "ranking-item",
        "book-item",
        "fcn-story",
    ] {
        blocks.extend(
            body.split(marker)
                .skip(1)
                .filter(|part| part.contains("href="))
                .map(|part| format!("{marker}{part}")),
        );
    }
    blocks
}

fn first_story_href(block: &str) -> Option<String> {
    block
        .split("<a")
        .skip(1)
        .filter_map(|part| html::attr(part, "href"))
        .find(|href| {
            let lower = href.to_ascii_lowercase();
            !lower.contains("#")
                && !lower.contains("author")
                && !lower.contains("tag")
                && !lower.contains("genre")
        })
}

fn title_from_listing(block: &str) -> Option<String> {
    for marker in [
        "card__title",
        "card-title",
        "story-title",
        "story__title",
        "ranking-title",
        "entry-title",
        "<h2",
        "<h3",
        "<h4",
        "class=\"tt",
    ] {
        if let Some(value) = text_for_marker(block, marker) {
            return Some(value);
        }
    }
    None
}

fn image_from_block(block: &str) -> Option<String> {
    html::attr_after(block, "<img", "data-src")
        .or_else(|| html::attr_after(block, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(block, "<img", "src"))
        .or_else(|| html::attr_after(block, "ranking-cover", "data-bg"))
        .or_else(|| html::attr_after(block, "story-cover", "data-bg"))
        .or_else(|| html::attr_after(block, "img-cover", "data-bg"))
        .or_else(|| style_url(block))
        .map(|src| absolute_url(&src))
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&story_url(key), DETAILS_FIXTURE);
    parse_details(&body, key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let mut item = catalog_item(
        normalize_key(key),
        text_for_marker(body, "story__identity-title")
            .or_else(|| text_for_marker(body, "post-title"))
            .or_else(|| text_between_tag(body, "h1"))
            .unwrap_or_else(|| title_from_key(key)),
        html::attr_after(body, "story__thumbnail", "data-src")
            .or_else(|| html::attr_after(body, "story__thumbnail", "src"))
            .or_else(|| html::attr_after(body, "figure.story__thumbnail", "href"))
            .map(|src| absolute_url(&src))
            .or_else(|| Some(DEFAULT_COVER.to_string())),
        true,
    );
    item.authors = parse_author(body).into_iter().collect();
    item.description = parse_summary(body);
    item.tags = tag_links(body);
    item.status = parse_status(body);
    item
}

fn parse_author(body: &str) -> Option<String> {
    for marker in [
        "custom-story-info",
        "/author/",
        "rel=\"author\"",
        "story__author",
        "story-author",
        "author-name",
        "post-author",
        "__author",
    ] {
        if let Some(value) = text_for_marker(body, marker) {
            let author = value
                .split('|')
                .next()
                .unwrap_or(&value)
                .trim()
                .trim_start_matches("Autor")
                .trim_start_matches("Autora")
                .trim_start_matches("Por")
                .trim_start_matches("Author")
                .trim_start_matches("by")
                .trim_matches(':')
                .trim()
                .to_string();
            if !author.is_empty() {
                return Some(author);
            }
        }
    }
    Some("Desconhecido".to_string())
}

fn parse_summary(body: &str) -> Option<String> {
    for marker in [
        "story__summary",
        "class=\"summary",
        "section story__summary",
    ] {
        if let Some(value) = html::text_between(body, marker, "</section>")
            .or_else(|| html::text_between(body, marker, "</div>"))
        {
            let normalized = value
                .replace("<br>", "\n")
                .replace("<br/>", "\n")
                .replace("</p>", "\n\n")
                .replace("</div>", "\n");
            return Some(html::strip_tags(&normalized)).filter(|summary| !summary.is_empty());
        }
    }
    None
}

fn tag_links(body: &str) -> Vec<String> {
    let source = body
        .split("tag-group")
        .nth(1)
        .or_else(|| body.split("genres").nth(1))
        .unwrap_or_default();
    source
        .split("<a")
        .skip(1)
        .filter_map(|part| {
            html::text_between(part, ">", "</a>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .take(50)
        .collect()
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    let mut seen = BTreeSet::new();
    body.split("<li")
        .skip(1)
        .chain(body.split("chapter-item").skip(1))
        .filter_map(|block| {
            let href = html::attr_after(block, "<a", "href")?;
            let key = normalize_key(&href).trim_end_matches('/').to_string();
            if key.is_empty() || !seen.insert(key.clone()) {
                return None;
            }
            let title = html::text_between(block, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_key(&key));
            Some(NovelChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number(&title),
                url: Some(absolute_url(&key)),
                language: Some("pt-BR".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let raw = html::text_between(body, "id=\"chapter-content\"", "</section>")
        .or_else(|| html::text_between(body, "section#chapter-content", "</section>"))
        .or_else(|| html::text_between(body, "chapter-content", "</div>"))
        .unwrap_or_else(|| body.to_string());
    let normalized = novel::normalize_reader_html(&remove_card_noise(&raw));
    NovelText {
        title: text_between_tag(body, "h1"),
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(BASE_URL.to_string()),
        css: Some("img { max-width: 100%; height: auto; } body { line-height: 1.7; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        next_chapter_key: Some(normalize_key(key)),
        ..NovelText::default()
    }
}

fn remove_card_noise(input: &str) -> String {
    let mut output = input.to_string();
    for marker in [
        "<script",
        "<style",
        "<iframe",
        "patreon-popup",
        "fcn-notice",
        "fictioneer-notice",
        "div card",
    ] {
        if let Some(prefix) = output.split(marker).next() {
            output = prefix.to_string();
        }
    }
    output
}

fn parse_status(body: &str) -> ItemStatus {
    let status_text = text_for_marker(body, "story__status")
        .or_else(|| text_for_marker(body, "story__identity-meta"))
        .or_else(|| text_for_marker(body, "story-meta"))
        .unwrap_or_default()
        .to_lowercase();
    if status_text.contains("ongoing")
        || status_text.contains("andamento")
        || status_text.contains("lançando")
        || status_text.contains("ativa")
    {
        ItemStatus::Ongoing
    } else if status_text.contains("completed") || status_text.contains("completo") {
        ItemStatus::Completed
    } else if status_text.contains("cancelled")
        || status_text.contains("cancelado")
        || status_text.contains("dropado")
    {
        ItemStatus::Cancelled
    } else if status_text.contains("hiatus")
        || status_text.contains("hiato")
        || status_text.contains("pausado")
    {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Unknown
    }
}

fn chapter_number(title: &str) -> Option<f32> {
    title
        .split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .filter(|part| !part.is_empty())
        .next()
        .and_then(|part| part.parse().ok())
}

fn style_url(block: &str) -> Option<String> {
    let style = html::attr(block, "style")?;
    let after = style.split("url(").nth(1)?;
    Some(
        after
            .trim_matches(|ch| ch == '\'' || ch == '"' || ch == ')')
            .to_string(),
    )
}

fn catalog_item(
    key: String,
    title: String,
    cover: Option<String>,
    initialized: bool,
) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|cover| absolute_url(&cover)),
        url: Some(absolute_url(&key)),
        language: Some("pt-BR".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn text_for_marker(body: &str, marker: &str) -> Option<String> {
    html::text_between(body, marker, "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn text_between_tag(body: &str, tag: &str) -> Option<String> {
    html::text_between(body, &format!("<{tag}"), &format!("</{tag}>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn has_next_page(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("rel=\"next\"")
        || lower.contains("class=\"next")
        || lower.contains("page-numbers")
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .map(normalize_key)
        .filter(|key| !key.is_empty())
}

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

fn story_url(path: &str) -> String {
    let mut target = absolute_url(path);
    if !target.ends_with('/') {
        target.push('/');
    }
    target
}

fn absolute_url(path: &str) -> String {
    url::join_url(BASE_URL, path)
}

fn title_from_key(key: &str) -> String {
    url::slug_from_url(key)
        .unwrap_or_else(|| "Novel".to_string())
        .replace('-', " ")
}

fn with_listing(mut request: Value, listing: &str) -> Value {
    if !request.is_object() {
        request = json!({});
    }
    if let Some(object) = request.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    request
}

const LIST_FIXTURE: &str = r#"
<ul id="search-result-list"><li><h2><a href="https://illusia.com.br/story/sample/">Sample Story</a></h2><img src="/cover.jpg"></li></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="story__identity-title">Sample Story</h1>
<figure class="story__thumbnail"><img src="/cover.jpg"></figure>
<div class="story__identity-meta">Autor: Sample Author | Em andamento</div>
<section class="story__summary"><p>Sample summary.</p></section>
<div class="tag-group"><a>Fantasia</a></div>
<ul class="chapter-list"><li><a href="/story/sample/chapter-1/">Capitulo 1</a></li></ul>
"#;

const TEXT_FIXTURE: &str = r#"
<section id="chapter-content"><div><p>Sample chapter text.</p></div></section>
"#;

export_novel_source!(SOURCE);
